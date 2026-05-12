# Hector A3 — Diff Pre-Filter for Semantic Engine

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or superpowers:subagent-driven-development) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec section:** [`specs/2026-05-12-bully-parity-closures.md` §A3](../specs/2026-05-12-bully-parity-closures.md)
**Severity:** 🔴 critical (cost lever)
**Sequencing:** Third item in the 0.2.0 cohort (after [A1](2026-05-12-hector-a1-prompt-injection.md), [A2](2026-05-12-hector-a2-skip-patterns.md)).

---

**Goal:** Locally short-circuit `engine: semantic` dispatch when the diff cannot meaningfully fire the rule — empty, whitespace-only, comment-only (unless the rule mentions comments), or pure-deletion against an "avoid X" rule. Most edits during a debugging session match one of these; each one we skip is one fewer paid LLM call.

**Architecture:** A new pure-function module `crates/hector-core/src/diff/analysis.rs` exposes `can_match_diff(diff: &str, file_path: &Path, rule_description: &str) -> CanMatch`. The runner calls it for every `EngineKind::Semantic` rule *before* dispatching to `SemanticEngine::run`. On `CanMatch::No(reason)` the runner records the rule as passed, writes a `kind: "semantic_skipped"` telemetry record with the reason, and never enters the engine. The engine itself stays pure — no telemetry, no fs access added. Comment detection uses a static file-extension → comment-marker map keyed by suffix; languages cover at least Rust, TS/JS, Python, Go, Ruby, shell. The "avoid" heuristic is a fixed list of word-boundary keywords (`avoid|don't|do not|no|ban|forbid|prohibit`, case-insensitive). `script` and `ast` engines are untouched.

**Tech Stack:** Rust, workspace-stable. No new deps — `regex` is already transitively available via `globset`/`ast-grep-core`, so we use it for the "avoid" word-boundary check. If a clean tree doesn't expose `regex` as a direct dep on `hector-core`, add it to `[dependencies]`. Tests use the existing `FakeLlm` pattern from `tests/semantic_engine.rs` plus a small call-counter wrapper to assert non-invocation.

---

## Decisions ratified up-front (per spec §3 + §A3)

| Decision | Choice | Reason |
|---|---|---|
| Filter applies to which engines | **Semantic only.** Script & AST are cheap and may legitimately fire on whitespace. | Spec §A3 step 5. Don't smuggle. |
| Where the check is called | **Runner**, not inside `SemanticEngine::run` | Runner already owns telemetry, scope, baseline, disable, and skip-pattern orchestration. Keeps the engine pure and the wiremock acceptance criterion ("no HTTP request reaches the mock for skipped diffs") trivially provable. Spec §A3 step 4 suggested the engine; we deviate for separation of concerns. Document this deviation in the per-feature plan (here). |
| Telemetry record shape | Extend flat `LogEntry` with `reason: Option<String>` (additive, `serde(skip_serializing_if)`-style). Use `kind: "semantic_skipped"`. | D1 (typed telemetry) hasn't shipped; same pattern as A2's `kind: "skipped"`. When D1 lands, this becomes the `SemanticSkipped` variant. |
| Comment-detection languages | Rust/C/C++/Java/Go/Swift (`//`, `/*`, `*`, `*/`), JS/TS (`//`, `/*`, `*`, `*/`), Python/Ruby/Shell/YAML/TOML/Make (`#`), Lua/Haskell/SQL (`--`), Lisp/Scheme (`;`), PHP (`//`, `#`, `/*`, `*`, `*/`). Match on file extension; unknown extensions return `CanMatch::Yes` (don't skip). | Spec calls out "at least Rust, TS/JS, Python, Go, Ruby, shell"; we cover that and a handful more cheap wins. Conservative on unknown extensions — false negatives (extra LLM call) are cheaper than false positives (missed violation). |
| Block-comment lines inside diffs | Best-effort line-by-line: `/*`, `*`, `*/`, `<!--`, `-->`, `//`, `#`, `--`, `;` as line-start prefixes after `trim_start`. We do not track block-comment open/close state across hunk boundaries. | Diffs only show ± lines, no surrounding state. A correct stateful parser would need the full file. Best-effort is fine: a multi-line block-comment edit whose interior lines start with `*` is recognized; an edit whose interior lines are arbitrary prose is not (we don't skip → LLM runs → correct behavior, just costs more). |
| "Avoid" keyword list | `avoid`, `don't`, `do not`, `no`, `ban`, `forbid`, `prohibit` (case-insensitive, word-boundary). | Spec verbatim. "no" is broad on purpose: rules phrased "no X" are by far the dominant authoring pattern, and false-positive skipping on pure-deletion diffs is benign (deletion can't introduce X anyway). |
| Rule description mentions "comment" | If `description` contains the word `comment` (case-insensitive, word-boundary), do **not** apply the comment-only skip — the rule may be specifically about comments. | Spec §A3 bullet 3. |
| Verdict shape | Unchanged. Skipped rules land in `passed_checks` exactly like rules whose LLM returned `Pass`. | Verdict locks at 0.3; this is additive-only. Status distinction is via telemetry, not verdict. |
| Trust fingerprint | Unaffected. The pre-filter is engine internals, not config. | Same reasoning as A2: fingerprint hashes the YAML, not the parser/runner. |

If anything in this table feels wrong on a fresh read, raise it before Task 3 — it sets the filter's behavior.

---

## File structure

```
crates/hector-core/
├── src/
│   ├── diff/
│   │   ├── mod.rs                  ← MODIFIED: pub mod analysis; re-export
│   │   └── analysis.rs             ← NEW: SkipReason, CanMatch, can_match_diff, comment maps, avoid regex
│   ├── telemetry.rs                ← MODIFIED: add `reason: Option<String>` to LogEntry (additive)
│   └── runner.rs                   ← MODIFIED: call can_match_diff before SemanticEngine dispatch; record telemetry on skip
└── tests/
    ├── diff_analysis.rs            ← NEW: per-SkipReason unit tests + extension map coverage
    └── runner_semantic_prefilter.rs ← NEW: integration — FakeLlm with call-counter never invoked for a skipped diff
```

The runner change in `runner.rs` lives in the existing per-rule loop (lines ~209–282). No new files in the engine module — the engine is intentionally untouched.

---

## Phase 1 — `diff/analysis.rs` module (TDD)

### Task 1: Failing unit tests for `can_match_diff`

**Files:**
- Create: `crates/hector-core/tests/diff_analysis.rs`

The module doesn't exist yet — tests will fail to compile. We add the test file with the full surface area we want; Task 2 makes it pass.

- [ ] **Step 1: Create the test file**

```rust
use hector_core::diff::analysis::{can_match_diff, CanMatch, SkipReason};
use std::path::Path;

const ADD_HELLO_RS: &str = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,1 +1,2 @@
 fn main() {}
+fn hello() {}
";

const WHITESPACE_ONLY_RS: &str = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,4 @@
 fn main() {}
+
+    
 fn other() {}
";

const COMMENT_ONLY_RS: &str = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,1 +1,3 @@
 fn main() {}
+// new comment
+/* block */
";

const COMMENT_ONLY_PY: &str = "\
--- a/app.py
+++ b/app.py
@@ -1,1 +1,2 @@
 x = 1
+# pep8 comment
";

const PURE_DELETION: &str = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,1 @@
 fn main() {}
-fn dead() {}
";

const EMPTY_DIFF: &str = "";

#[test]
fn empty_diff_is_skipped() {
    let r = can_match_diff(EMPTY_DIFF, Path::new("src/lib.rs"), "no panic in library code");
    assert!(matches!(r, CanMatch::No(SkipReason::Empty)));
}

#[test]
fn whitespace_only_diff_is_skipped() {
    let r = can_match_diff(WHITESPACE_ONLY_RS, Path::new("src/lib.rs"), "no panic in library code");
    assert!(matches!(r, CanMatch::No(SkipReason::WhitespaceOnly)));
}

#[test]
fn comment_only_rs_diff_is_skipped_when_rule_not_about_comments() {
    let r = can_match_diff(COMMENT_ONLY_RS, Path::new("src/lib.rs"), "no unwrap in library code");
    assert!(matches!(r, CanMatch::No(SkipReason::CommentsOnly)));
}

#[test]
fn comment_only_py_diff_is_skipped() {
    let r = can_match_diff(COMMENT_ONLY_PY, Path::new("app.py"), "no print statements");
    assert!(matches!(r, CanMatch::No(SkipReason::CommentsOnly)));
}

#[test]
fn comment_only_diff_is_not_skipped_when_rule_mentions_comments() {
    // Rule is specifically about comments — must dispatch.
    let r = can_match_diff(COMMENT_ONLY_RS, Path::new("src/lib.rs"), "no TODO comments left behind");
    assert!(matches!(r, CanMatch::Yes));
}

#[test]
fn pure_deletion_skipped_for_avoid_rule() {
    let r = can_match_diff(PURE_DELETION, Path::new("src/lib.rs"), "avoid panics in lib code");
    assert!(matches!(r, CanMatch::No(SkipReason::PureDeletion)));
}

#[test]
fn pure_deletion_skipped_for_no_x_rule() {
    let r = can_match_diff(PURE_DELETION, Path::new("src/lib.rs"), "no eprintln! in lib code");
    assert!(matches!(r, CanMatch::No(SkipReason::PureDeletion)));
}

#[test]
fn pure_deletion_skipped_for_dont_x_rule() {
    let r = can_match_diff(PURE_DELETION, Path::new("src/lib.rs"), "don't call unwrap");
    assert!(matches!(r, CanMatch::No(SkipReason::PureDeletion)));
}

#[test]
fn pure_deletion_dispatched_for_positive_rule() {
    // "Functions must have docs" is a positive requirement, not an "avoid".
    // A pure deletion could still remove docs from a function -> must dispatch.
    let r = can_match_diff(PURE_DELETION, Path::new("src/lib.rs"), "functions must have docs");
    assert!(matches!(r, CanMatch::Yes));
}

#[test]
fn real_addition_diff_is_dispatched() {
    let r = can_match_diff(ADD_HELLO_RS, Path::new("src/lib.rs"), "no useEffect derives state");
    assert!(matches!(r, CanMatch::Yes));
}

#[test]
fn unknown_extension_with_comment_chars_is_dispatched() {
    // We don't recognize `.xyz`; safer to dispatch than mis-skip.
    let diff = "\
--- a/foo.xyz
+++ b/foo.xyz
@@ -1,1 +1,2 @@
 a
+// looks like a comment but we don't know this language
";
    let r = can_match_diff(diff, Path::new("foo.xyz"), "no foo");
    assert!(matches!(r, CanMatch::Yes));
}

#[test]
fn mixed_comment_and_code_is_dispatched() {
    let diff = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,1 +1,3 @@
 fn main() {}
+// helper
+fn hello() {}
";
    let r = can_match_diff(diff, Path::new("src/lib.rs"), "no helpers");
    assert!(matches!(r, CanMatch::Yes));
}
```

- [ ] **Step 2: Run tests, confirm they fail to compile**

Run: `cargo test --test diff_analysis 2>&1 | head -20`

Expected: compile error — unresolved import `hector_core::diff::analysis`.

### Task 2: Implement the analysis module

**Files:**
- Create: `crates/hector-core/src/diff/analysis.rs`
- Modify: `crates/hector-core/src/diff/mod.rs:1-3` (add `pub mod analysis;`)
- Possibly modify: `crates/hector-core/Cargo.toml` (add `regex` to `[dependencies]` if not already a direct dep)

- [ ] **Step 1: Confirm `regex` availability**

Run: `cargo tree -p hector-core --depth 1 | grep -i regex`

If absent from direct deps, run: `cargo add -p hector-core regex` (no version pin — use whatever the workspace converges on).

- [ ] **Step 2: Add module declaration**

Edit `crates/hector-core/src/diff/mod.rs`:

```rust
pub mod analysis;
pub mod parser;

pub use parser::{parse_unified, ChangedFile};
```

- [ ] **Step 3: Implement `analysis.rs`**

```rust
//! Local diff analysis to short-circuit expensive semantic dispatch.
//!
//! `can_match_diff` answers a single question: given this diff, this file, and
//! this rule description, is it *possible* for the semantic engine to find a
//! violation? On `No(reason)`, the runner skips the LLM call entirely.
//!
//! This is a cost lever, not a correctness gate. False negatives (we say
//! `Yes` when the LLM would have passed) just mean the LLM runs anyway —
//! same as no filter. False positives (we say `No` when the LLM would have
//! flagged a violation) are silent misses, so each `No` branch errs
//! conservative: unknown extensions, unrecognized "avoid" phrasings, and
//! mixed comment-and-code all dispatch.

use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// No `@@` hunks in the diff body. Most commonly a file-rename-only diff.
    Empty,
    /// At least one `+` line, but every `+` line is blank or whitespace-only.
    WhitespaceOnly,
    /// Every `+` line is a comment in the file's language, AND the rule
    /// description does not mention "comment". Mixed comment+code dispatches.
    CommentsOnly,
    /// No `+` lines at all, ≥1 `-` line, AND the rule description matches the
    /// "avoid X" heuristic — a pure deletion cannot introduce anything new
    /// to be avoided.
    PureDeletion,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SkipReason::Empty => "empty",
            SkipReason::WhitespaceOnly => "whitespace_only",
            SkipReason::CommentsOnly => "comments_only",
            SkipReason::PureDeletion => "pure_deletion",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanMatch {
    Yes,
    No(SkipReason),
}

/// Decide whether the semantic engine could plausibly find a violation in
/// this diff for a rule with this description. See module docs for the
/// false-negative / false-positive contract.
pub fn can_match_diff(diff: &str, file_path: &Path, rule_description: &str) -> CanMatch {
    let lines: Vec<&str> = diff.lines().collect();
    let mut in_hunk = false;
    let mut added: Vec<&str> = Vec::new();
    let mut removed_count: usize = 0;

    for raw in &lines {
        if raw.starts_with("@@ ") || raw.starts_with("@@\t") {
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue;
        }
        // Inside a hunk now. Skip the file-header lines that can appear
        // between hunks.
        if raw.starts_with("+++") || raw.starts_with("---") {
            continue;
        }
        if let Some(content) = raw.strip_prefix('+') {
            added.push(content);
        } else if raw.starts_with('-') {
            removed_count += 1;
        }
    }

    if !in_hunk {
        return CanMatch::No(SkipReason::Empty);
    }

    if added.is_empty() {
        // Pure deletion. Skip only if the rule reads as "avoid X".
        if removed_count > 0 && is_avoid_rule(rule_description) {
            return CanMatch::No(SkipReason::PureDeletion);
        }
        return CanMatch::Yes;
    }

    if added.iter().all(|l| l.trim().is_empty()) {
        return CanMatch::No(SkipReason::WhitespaceOnly);
    }

    if let Some(markers) = comment_markers_for(file_path) {
        let all_comments = added
            .iter()
            .all(|l| {
                let t = l.trim_start();
                t.is_empty() || markers.iter().any(|m| t.starts_with(m))
            });
        if all_comments && !rule_mentions_comments(rule_description) {
            return CanMatch::No(SkipReason::CommentsOnly);
        }
    }

    CanMatch::Yes
}

/// Per-extension line-start prefixes that mark a line as a comment.
/// Block-comment lines (`/* foo`, ` * bar`, ` */`) are detected by their
/// leading prefix after `trim_start`; we don't track open/close state.
fn comment_markers_for(path: &Path) -> Option<&'static [&'static str]> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        // C-family: line + block start/mid/end
        "rs" | "c" | "h" | "cc" | "cpp" | "hpp" | "java" | "swift" | "kt" | "kts" | "scala"
        | "cs" | "go" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => &["//", "/*", "*/", "*"],
        // PHP: C-family plus shell-style
        "php" => &["//", "#", "/*", "*/", "*"],
        // Hash-prefix family
        "py" | "rb" | "sh" | "bash" | "zsh" | "fish" | "yml" | "yaml" | "toml" | "ini" | "cfg"
        | "conf" | "mk" | "makefile" | "dockerfile" | "gitignore" => &["#"],
        // Dash-prefix family
        "lua" | "hs" | "sql" | "ada" | "adb" | "ads" => &["--"],
        // Semicolon-prefix family
        "lisp" | "lsp" | "el" | "scm" | "clj" | "cljs" | "cljc" => &[";"],
        // HTML/XML
        "html" | "htm" | "xml" | "svg" | "vue" | "svelte" => &["<!--", "-->"],
        _ => return None,
    })
}

fn is_avoid_rule(description: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)\b(avoid|don't|do not|no|ban|forbid|prohibit)\b").unwrap()
    });
    re.is_match(description)
}

fn rule_mentions_comments(description: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)\bcomments?\b").unwrap());
    re.is_match(description)
}
```

- [ ] **Step 4: Run the tests, confirm they pass**

Run: `cargo test --test diff_analysis`

Expected: all 11 tests pass.

- [ ] **Step 5: Confirm no other tests regressed**

Run: `cargo test -p hector-core`

Expected: full crate green.

- [ ] **Step 6: Commit**

```bash
git add crates/hector-core/src/diff/mod.rs \
        crates/hector-core/src/diff/analysis.rs \
        crates/hector-core/tests/diff_analysis.rs \
        crates/hector-core/Cargo.toml
# If Cargo.toml unchanged (regex already a direct dep), drop it from the add.
git commit -m "feat(diff): add can_match_diff pre-filter for semantic engine (A3 phase 1)"
```

---

## Phase 2 — Telemetry schema additive change

### Task 3: Add `reason: Option<String>` to `LogEntry`

**Files:**
- Modify: `crates/hector-core/src/telemetry.rs:7-15`
- Modify: `crates/hector-core/tests/telemetry.rs` (one new test case)

The current `LogEntry` is a flat struct serialized with serde. Adding an `Option<String>` field with `#[serde(skip_serializing_if = "Option::is_none")]` is wire-compatible: old logs continue to parse (Option defaults to None on absence); new logs only emit the field when present.

- [ ] **Step 1: Write the failing test**

Append to `crates/hector-core/tests/telemetry.rs`:

```rust
#[test]
fn log_entry_with_reason_serializes_field() {
    let dir = tempdir().unwrap();
    let log = dir.path().join(".hector/log.jsonl");
    let entry = LogEntry {
        timestamp: "2026-05-12T00:00:00Z".into(),
        kind: "semantic_skipped".into(),
        file: "src/lib.rs".into(),
        rule_id: Some("no-unwrap".into()),
        status: "pass".into(),
        elapsed_ms: 0,
        reason: Some("whitespace_only".into()),
    };
    append(&log, &entry).unwrap();
    let content = std::fs::read_to_string(&log).unwrap();
    assert!(content.contains("\"reason\":\"whitespace_only\""));
    assert!(content.contains("\"kind\":\"semantic_skipped\""));
}

#[test]
fn log_entry_without_reason_omits_field() {
    let dir = tempdir().unwrap();
    let log = dir.path().join(".hector/log.jsonl");
    let entry = LogEntry {
        timestamp: "2026-05-12T00:00:01Z".into(),
        kind: "check".into(),
        file: "src/lib.rs".into(),
        rule_id: None,
        status: "pass".into(),
        elapsed_ms: 1,
        reason: None,
    };
    append(&log, &entry).unwrap();
    let content = std::fs::read_to_string(&log).unwrap();
    assert!(!content.contains("\"reason\""));
}
```

- [ ] **Step 2: Run, confirm both fail to compile**

Run: `cargo test --test telemetry log_entry_with_reason 2>&1 | head -10`

Expected: error — missing field `reason` (existing call sites) and unknown field `reason` (new test sees a not-yet-added struct field).

- [ ] **Step 3: Add the field**

Edit `crates/hector-core/src/telemetry.rs`:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub kind: String,
    pub file: String,
    pub rule_id: Option<String>,
    pub status: String,
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
```

- [ ] **Step 4: Fix the three existing `LogEntry { … }` construction sites**

Edit `crates/hector-core/src/runner.rs` lines 188, 313, 393 (the three telemetry appends): add `reason: None,` at the end of each struct literal.

Also fix the existing tests in `crates/hector-core/tests/telemetry.rs` that construct `LogEntry` (`append_creates_log_and_writes_jsonl` and any others) — add `reason: None,` to those literals.

- [ ] **Step 5: Run the full crate**

Run: `cargo test -p hector-core`

Expected: green. Confirm the two new telemetry tests pass and the existing telemetry/runner tests still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/hector-core/src/telemetry.rs \
        crates/hector-core/src/runner.rs \
        crates/hector-core/tests/telemetry.rs
git commit -m "feat(telemetry): add optional reason field to LogEntry (A3 phase 2)"
```

---

## Phase 3 — Wire the pre-filter into the runner

### Task 4: Skip semantic dispatch when `can_match_diff` says no

**Files:**
- Modify: `crates/hector-core/src/runner.rs` (the per-rule loop, ~lines 209–282)

The loop currently looks like:

```rust
for (rule_id, rule) in &self.config.rules {
    let matcher = ScopeMatcher::new(&rule.scope).expect("scope validated at load");
    if !matcher.matches(&match_path) { continue; }
    let ctx = RuleContext { … };
    let outcome: Result<Vec<Violation>> = match rule.engine {
        EngineKind::Script => …,
        EngineKind::Ast => …,
        EngineKind::Semantic => crate::engine::semantic::SemanticEngine.run(&ctx),
        _ => Ok(Vec::new()),
    };
    …
}
```

We insert a pre-filter step for the `Semantic` arm only: build the rule context as today, but **before** entering the `match`, if the rule's engine is `Semantic` and `can_match_diff(&diff, &path, &rule.description)` returns `CanMatch::No(reason)`, append a `semantic_skipped` telemetry record, push the rule into `passed` (so it shows up in `passed_checks` and downstream behavior matches a real pass), and `continue`.

- [ ] **Step 1: Read the current loop once**

Open `crates/hector-core/src/runner.rs` around lines 200–290 to verify the exact shape before editing.

- [ ] **Step 2: Insert the pre-filter**

Replace the block starting at the `for (rule_id, rule) in &self.config.rules {` line and ending immediately before `let outcome: Result<Vec<Violation>> = match rule.engine {` with the following. (Keep the existing variable names and surrounding code unchanged.)

```rust
for (rule_id, rule) in &self.config.rules {
    let matcher = crate::config::scope::ScopeMatcher::new(&rule.scope)
        .expect("scope validated at load");
    if !matcher.matches(&match_path) {
        continue;
    }

    // A3: short-circuit expensive semantic dispatch when the diff cannot
    // plausibly match. Script and AST engines are cheap and may
    // legitimately fire on whitespace/comments — leave them alone.
    if rule.engine == EngineKind::Semantic {
        let analysis = crate::diff::analysis::can_match_diff(
            &diff,
            &path,
            &rule.description,
        );
        if let crate::diff::analysis::CanMatch::No(reason) = analysis {
            if let Err(e) = crate::telemetry::append(
                &self.config_dir.join(".hector/log.jsonl"),
                &crate::telemetry::LogEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    kind: "semantic_skipped".into(),
                    file: path.display().to_string(),
                    rule_id: Some(rule_id.clone()),
                    status: "pass".into(),
                    elapsed_ms: 0,
                    reason: Some(reason.as_str().to_string()),
                },
            ) {
                eprintln!("hector: telemetry append failed: {e:#}");
            }
            passed.push(rule_id.clone());
            continue;
        }
    }

    let ctx = RuleContext {
        rule_id,
        rule,
        file: &path,
        content: if content.is_empty() { None } else { Some(&content) },
        diff: if diff.is_empty() { None } else { Some(&diff) },
        cwd: &self.config_dir,
        llm: self.llm.as_deref(),
    };
```

(Leave the existing `let outcome: Result<Vec<Violation>> = match rule.engine { … }` and downstream handling unchanged.)

- [ ] **Step 3: Compile**

Run: `cargo build -p hector-core`

Expected: green.

- [ ] **Step 4: Run the existing test suite as a smoke test**

Run: `cargo test -p hector-core`

Expected: green. If `runner_diff.rs` or `semantic_engine.rs` has a test that fed a comment-only or whitespace-only diff to the semantic engine and asserted a verdict, that test will now find the rule in `passed_checks` (because the pre-filter routed it there). Update the assertion to match — the rule is still "passed", just by a different path.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-core/src/runner.rs
git commit -m "feat(runner): apply diff pre-filter before semantic dispatch (A3 phase 3)"
```

---

## Phase 4 — Integration test: LLM never invoked for skipped diff

### Task 5: Wiremock-equivalent assertion via a call-counter `LlmClient`

**Files:**
- Create: `crates/hector-core/tests/runner_semantic_prefilter.rs`

We use a `FakeLlm` that records every `evaluate` call. The acceptance criterion ("verified by wiremock: no request reaches the mock") is functionally equivalent and avoids spinning up a real HTTP server.

- [ ] **Step 1: Write the test**

The real public surface (verified against `crates/hector-core/src/runner.rs:87-89` and `tests/runner_diff.rs`) is `HectorEngine::check(CheckInput::Diff { file, unified_diff })` and `trust::write_trust_block(raw_yaml)` for trust-stamping a config inline.

```rust
use anyhow::Result;
use hector_core::config::Rule;
use hector_core::llm::{LlmClient, RuleVerdict};
use hector_core::runner::{CheckInput, HectorEngine};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::tempdir;

struct CountingLlm {
    calls: Arc<AtomicUsize>,
}

impl LlmClient for CountingLlm {
    fn evaluate(
        &self,
        _rules: &[(&str, &Rule)],
        _primary: &str,
        _context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // Should never be reached in the skip test; return a no-violation
        // verdict so the dispatch test sees a clean pass.
        Ok(vec![RuleVerdict {
            rule_id: "no-unwrap".to_string(),
            status: hector_core::llm::RuleStatus::Pass,
        }])
    }
}

fn write_trusted_config(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    let body = r#"schema_version: 2
rules:
  no-unwrap:
    description: "no unwrap in library code"
    engine: semantic
    scope:
      - "**/*.rs"
    severity: warning
    context: diff
"#;
    std::fs::write(&path, body).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let with_trust = hector_core::trust::write_trust_block(&raw).unwrap();
    std::fs::write(&path, with_trust).unwrap();
    path
}

#[test]
fn whitespace_only_diff_does_not_dispatch_llm() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted_config(dir.path());
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main() {}\n   \n").unwrap();

    let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,2 @@
 fn main() {}
+   
";

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = HectorEngine::builder()
        .with_llm(Box::new(CountingLlm { calls: calls.clone() }))
        .load(&cfg)
        .unwrap();

    let verdict = engine
        .check(CheckInput::Diff { file: file.clone(), unified_diff: diff.to_string() })
        .unwrap();

    assert_eq!(
        calls.load(Ordering::SeqCst), 0,
        "LLM must not be invoked for whitespace-only diff"
    );
    assert!(
        verdict.passed_checks.iter().any(|id| id == "no-unwrap"),
        "skipped rule should land in passed_checks; got {:?}", verdict.passed_checks
    );
    assert!(verdict.violations.is_empty());
}

#[test]
fn real_addition_diff_dispatches_llm() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted_config(dir.path());
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main() {}\nfn hello() {}\n").unwrap();

    let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,2 @@
 fn main() {}
+fn hello() {}
";

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = HectorEngine::builder()
        .with_llm(Box::new(CountingLlm { calls: calls.clone() }))
        .load(&cfg)
        .unwrap();

    let _ = engine
        .check(CheckInput::Diff { file, unified_diff: diff.to_string() })
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst), 1,
        "LLM must be invoked once for real addition"
    );
}

#[test]
fn pure_deletion_against_avoid_rule_does_not_dispatch_llm() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted_config(dir.path());
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();

    let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,2 +1,1 @@
 fn main() {}
-fn dead() {}
";

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = HectorEngine::builder()
        .with_llm(Box::new(CountingLlm { calls: calls.clone() }))
        .load(&cfg)
        .unwrap();

    let _ = engine
        .check(CheckInput::Diff { file, unified_diff: diff.to_string() })
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst), 0,
        "LLM must not be invoked for pure-deletion against an 'avoid' rule"
    );
}

#[test]
fn semantic_skipped_telemetry_recorded() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted_config(dir.path());
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main() {}\n   \n").unwrap();

    let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,2 @@
 fn main() {}
+   
";

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = HectorEngine::builder()
        .with_llm(Box::new(CountingLlm { calls: calls.clone() }))
        .load(&cfg)
        .unwrap();
    let _ = engine
        .check(CheckInput::Diff { file, unified_diff: diff.to_string() })
        .unwrap();

    let log = std::fs::read_to_string(dir.path().join(".hector/log.jsonl")).unwrap();
    assert!(
        log.contains("\"kind\":\"semantic_skipped\""),
        "telemetry missing semantic_skipped: {log}"
    );
    assert!(
        log.contains("\"reason\":\"whitespace_only\""),
        "telemetry missing reason: {log}"
    );
    assert!(
        log.contains("\"rule_id\":\"no-unwrap\""),
        "telemetry missing rule_id: {log}"
    );
}
```

- [ ] **Step 2: Run the four tests**

Run: `cargo test --test runner_semantic_prefilter`

Expected: all four pass.

- [ ] **Step 3: Run the full crate suite once more**

Run: `cargo test -p hector-core`

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/hector-core/tests/runner_semantic_prefilter.rs
git commit -m "test(runner): assert LLM not dispatched for skipped diffs (A3 phase 4)"
```

---

## Phase 5 — Lint + format + final cross-check

### Task 6: Sweep

- [ ] **Step 1: Lint**

Run: `cargo clippy --all-targets -- -D warnings`

Expected: green. Common warnings to expect on new code:
- `clippy::needless_borrow` on the regex args — fix inline.
- `clippy::needless_collect` on the `lines: Vec<&str>` — leave it; we iterate twice indirectly.

- [ ] **Step 2: Format**

Run: `cargo fmt`

- [ ] **Step 3: Full workspace test**

Run: `cargo test`

Expected: green across workspace (including CLI tests that haven't been touched).

- [ ] **Step 4: Spec acceptance criteria pass**

Open `specs/2026-05-12-bully-parity-closures.md` §A3 and confirm:
- [ ] Fixture-driven tests for each `SkipReason` — covered by `tests/diff_analysis.rs`.
- [ ] A semantic rule on a pure-deletion diff returns without invoking the LLM — covered by the pure-deletion test in `tests/runner_semantic_prefilter.rs` (extend if not already present; the current test only covers whitespace).
- [ ] Comment detection covers at least Rust, TS/JS, Python, Go, Ruby, shell — covered by the per-extension map; add one explicit test per language if any are missing.
- [ ] Telemetry log shows `semantic_skipped` with reason for each skip — covered by `semantic_skipped_telemetry_recorded`.

If any acceptance criterion is uncovered, add a test in the appropriate file before committing.

- [ ] **Step 5: Commit any sweep fixes**

```bash
git add -A
git commit -m "style(diff): clippy + fmt sweep (A3 phase 5)"
```

(If nothing changed, skip this commit.)

---

## Test plan summary

| Test file | Covers |
|---|---|
| `tests/diff_analysis.rs` | All `SkipReason` variants; per-language comment markers; "avoid" heuristic positive + negative cases; unknown extension dispatches; mixed comment+code dispatches. |
| `tests/telemetry.rs` (extended) | `reason` field serializes when present, is omitted when None (no wire-shape change for existing entries). |
| `tests/runner_semantic_prefilter.rs` | End-to-end: counting LLM client confirms zero `evaluate` calls for a skipped diff; one call for a real addition; `kind: "semantic_skipped"` record lands in `.hector/log.jsonl` with the right reason and rule_id. |

---

## Risk / rollback

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| False-positive skip (we say `No`, real LLM would have flagged a violation) | low | medium — silent miss | Comment detection only fires when the rule description does not mention "comment"; pure-deletion only fires when description matches the "avoid" keywords. Unknown extensions never skip. |
| "no" word-boundary causes over-eager pure-deletion skipping | low | low — only affects pure-deletion diffs, which can't introduce violations | Documented in the decision table. Users hitting this can phrase rules differently or rely on session/AST engines. |
| Verdict schema impact | none | n/a | Verdict shape unchanged; skipped rules land in `passed_checks` exactly like real passes. |
| Telemetry schema impact | additive only | n/a — additive | `reason: Option<String>` with `skip_serializing_if`; existing log readers see no new field on existing records. |
| Trust fingerprint impact | none | n/a | Filter is engine internals; fingerprint hashes YAML. |
| Performance impact | net positive | n/a | Adds one pass over the diff string per semantic rule. Cost: O(diff length). Saved: one HTTP round-trip on skip. |
| Backwards compatibility | full | n/a | No public API change. `can_match_diff` is new public surface but additive. |

**Rollback:** revert the runner edit in Phase 3 (Task 4). The `diff::analysis` module becomes dead code but doesn't break anything. The `reason` field on `LogEntry` is harmless if unused.

---

## Self-review checklist (run before handing off)

- [ ] Every `SkipReason` variant has at least one fixture test.
- [ ] At least one test per spec-required language (Rust, TS/JS, Python, Go, Ruby, shell) — even if just confirming the extension-map lookup.
- [ ] Telemetry `reason` field serializes when present, omits when absent.
- [ ] `cargo clippy --all-targets -- -D warnings` is green.
- [ ] `cargo fmt` produced no diff after the final commit.
- [ ] `cargo test` passes across the workspace.
- [ ] No existing test was deleted to make a new behavior pass; if a test changed assertion, the change is justified in the commit message.
- [ ] The runner pre-filter only fires for `EngineKind::Semantic` — confirm with a grep that `EngineKind::Script` and `EngineKind::Ast` arms are unchanged.

---

## Hand-off

**Branching:** the current working branch (`bug-audit-remediation`) has unrelated WIP. Recommendation: commit or stash that work first, then branch `feat/a3-diff-prefilter` from `main` (or a fresh worktree off `main` per `superpowers:using-git-worktrees`). All five tasks are sequential — no parallelism payoff.

**Estimated effort:** 5 tasks, ~30 minutes each at a steady pace. Phase 2 is the only one that touches multiple files outside the new module (`runner.rs` × 3 call sites); the rest are localized.

**Follow-ups out of scope here:**
- C4 `--rule` / `--explain` / `--print-prompt` will eventually surface "skipped — reason" in the explain output; that consumes the telemetry kind landed here.
- D1 typed telemetry will replace the flat `LogEntry { reason: Option<String> }` with a tagged variant. The migration is bounded — `semantic_skipped` is the only consumer we add in A3.
- A4 `context.lines` is a separate prompt-shape change and does not interact with A3.
