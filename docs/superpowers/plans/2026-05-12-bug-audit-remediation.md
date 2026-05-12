# Hector 0.1 Bug Audit Remediation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. CLAUDE.md rule: every bugfix starts with a failing test — the failing test becomes the regression coverage.

**Goal:** Land fixes for all 44 findings in `docs/2026-05-12-bug-audit.md` (10 P0 + 12 P1 + 22 P2), ordered so production breakage and the RCE chain land first, the verdict-shape changes land before the 0.3 freeze, and P2 cleanup ships incrementally.

**Architecture:** Phase-ordered. Phase 0 unblocks every downstream phase. Phases 1–4 close P0s. Phase 5 settles the verdict shape before freeze. Phases 6–7 close P1s. Phase 8 is parallel P2 cleanup. Within each phase, tasks are tagged `[parallel]` or `[serial]` based on file-conflict graph so a coordinator can fan out subagents safely.

**Tech Stack:** Rust workspace (`hector-core` + `hector-cli`), `globset`, `ast-grep-core`, `reqwest` blocking, `nix` (Linux-only), `insta` snapshots, `assert_cmd` for CLI integration, `wiremock` for HTTP, bash + TypeScript adapters.

---

## Parallelism Map

The Mermaid graph below summarizes which phases gate which, plus intra-phase parallelism. The "fan-out" column tells a coordinator how many subagents to dispatch concurrently.

| Phase | Fan-out | Gates                      | Why                                                                                           |
|-------|---------|----------------------------|-----------------------------------------------------------------------------------------------|
| 0     | 1       | every later phase          | scope fix is the prerequisite for any test that uses absolute paths                           |
| 1     | 3       | Phase 2 (overlap on diff)  | three distinct files: `extends.rs`, `script.rs`, `diff/parser.rs`                             |
| 2     | 1       | Phase 5                    | a single coordinated refactor of `runner.rs` diff mode + `commands/check.rs` aggregation       |
| 3     | 1       | none                       | one file (`engine/capability.rs`) — sequential within                                          |
| 4     | 1       | none                       | one file (`commands/baseline.rs`)                                                              |
| 5     | 2       | freeze gate                | verdict enum + column/context population are disjoint                                          |
| 6     | 4       | none                       | four disjoint files: `disable.rs`/`runner.rs`, `llm/mod.rs`, semantic+session, `capability.rs` |
| 7     | 4       | none                       | LLM clients, adapters, telemetry, `engine/ast.rs` — all disjoint                               |
| 8     | up to 6 | none                       | 22 small fixes spread across many files                                                        |

Phases 0→1→2 must be strictly serial because they all touch `runner.rs`. Phases 3, 4, 5, 6, 7 may overlap if the executor is willing to manage merge conflicts.

---

## File-Touch Map

Phase 0:
- Modify: `crates/hector-core/src/config/scope.rs:5-32`
- Modify: `crates/hector-core/src/runner.rs:115` (and the matcher construction at `runner.rs:73-77`)
- Test: `crates/hector-core/tests/scope.rs`, new `crates/hector-core/tests/runner_absolute_path.rs`

Phase 1:
- Modify: `crates/hector-core/src/config/extends.rs:13-32`
- Modify: `crates/hector-core/src/runner.rs:58-71`
- Modify: `crates/hector-core/src/engine/script.rs:44`, `crates/hector-core/src/engine/capability.rs:35-36,75-77`
- Modify: `crates/hector-core/src/diff/parser.rs:11-55`
- Test: `crates/hector-core/tests/extends.rs`, `crates/hector-core/tests/e2e_script_rules.rs`, `crates/hector-core/tests/diff_parse.rs`

Phase 2:
- Modify: `crates/hector-core/src/runner.rs:120-253` (diff branch refactor)
- Modify: `crates/hector-cli/src/commands/check.rs:35-58`
- Test: `crates/hector-core/tests/runner_diff.rs`, `crates/hector-cli/tests/cli_check.rs`

Phase 3:
- Modify: `crates/hector-core/src/engine/capability.rs:25-65`
- Optional: `crates/hector-core/src/config/types.rs:98-105` (schema if removing variants)
- Modify: `docs/security.md`, `CLAUDE.md` (truth-in-advertising)
- Test: `crates/hector-core/tests/capability.rs`

Phase 4:
- Modify: `crates/hector-cli/src/commands/baseline.rs:6-49`
- Modify: `crates/hector-cli/Cargo.toml` (add `ignore`, `rayon` crates)
- Test: `crates/hector-cli/tests/cli_baseline.rs`

Phase 5:
- Modify: `crates/hector-core/src/verdict.rs:3,43-51,65-86`
- Modify: `crates/hector-core/src/runner.rs:222,278`
- Modify: every engine's Violation construction (column/context decision)
- Test: `crates/hector-core/tests/verdict_snapshot.rs`

Phase 6:
- Modify: `crates/hector-core/src/disable.rs:21-26,56-58`
- Modify: `crates/hector-core/src/runner.rs:209-214`
- Modify: `crates/hector-core/src/llm/mod.rs:128-138`
- Modify: `crates/hector-core/src/engine/semantic.rs:19-21`, `crates/hector-core/src/engine/session.rs:24-26`
- Modify: `crates/hector-core/src/engine/capability.rs:51` (timeout/output cap)
- Modify: `crates/hector-core/Cargo.toml` (add `wait-timeout` or `tokio::time`)
- Test: each in its existing test file

Phase 7:
- Modify: `crates/hector-core/src/llm/anthropic.rs:25`, `crates/hector-core/src/llm/openai_compat.rs:35`
- Modify: `adapters/claude-code/hooks/hook.sh:83-87`, `adapters/opencode/src/index.ts:126-130`
- Modify: `crates/hector-core/src/telemetry.rs:17-25`
- Modify: `crates/hector-core/src/engine/ast.rs:13-69`, `crates/hector-core/src/engine/mod.rs` (signature change)
- Test: `crates/hector-core/tests/anthropic.rs`, `tests/openai_compat.rs`, `tests/ast_engine.rs`, `tests/telemetry.rs`, `adapters/{claude-code,opencode}/tests/*`

Phase 8: see "Phase 8 — P2 Cleanup" — 22 items spread across many small files.

---

## Phase 0 — Scope Match (P0-1)

Turns Hector from "no-op in production" to "rules fire." Single-fix phase. Sequential gate for everything else.

### Task 0.1: Make scope match work for absolute input paths

**Files:**
- Modify: `crates/hector-core/src/config/scope.rs:5-32`
- Modify: `crates/hector-core/src/runner.rs:155-162` (the loop that calls `matcher.matches(&path)`)
- Test: `crates/hector-core/tests/scope.rs` (new test), new file `crates/hector-core/tests/runner_absolute_path.rs`

- [ ] **Step 1: Write the failing test** in `crates/hector-core/tests/scope.rs`. Append:

```rust
#[test]
fn scope_matcher_matches_relative_when_input_is_absolute() {
    use hector_core::config::scope::ScopeMatcher;
    let m = ScopeMatcher::new(&["crates/*/src/**/*.rs".to_string()]).unwrap();
    // The shipped behavior: an absolute path with the same suffix must match.
    let abs = std::path::PathBuf::from("/Users/anyone/work/hector/crates/hector-core/src/runner.rs");
    assert!(m.matches(&abs), "absolute path must match relative glob");
    let rel = std::path::PathBuf::from("crates/hector-core/src/runner.rs");
    assert!(m.matches(&rel), "relative path must still match");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p hector-core --test scope scope_matcher_matches_relative_when_input_is_absolute -- --nocapture`
Expected: FAIL — assertion fails on the absolute case.

- [ ] **Step 3: Add a runner-level integration test** that proves real `HectorEngine::check` fires for an absolute path. Create `crates/hector-core/tests/runner_absolute_path.rs`:

```rust
use hector_core::runner::{CheckInput, HectorEngine};
use std::fs;
use tempfile::tempdir;

#[test]
fn check_fires_for_absolute_input_path() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("src")).unwrap();
    let target = root.join("src/foo.rs");
    fs::write(&target, "fn main() {}\n").unwrap();
    let cfg = r#"schema_version: 2
rules:
  test-rule:
    description: must fire
    engine: script
    scope: ["src/**/*.rs"]
    severity: warning
    script: "exit 1"
"#;
    let cfg_path = root.join(".hector.yml");
    fs::write(&cfg_path, cfg).unwrap();
    let trusted = hector_core::trust::write_trust_block(cfg).unwrap();
    fs::write(&cfg_path, trusted).unwrap();

    let engine = HectorEngine::load(&cfg_path).unwrap();
    let verdict = engine
        .check(CheckInput::File {
            path: target.clone(),
            content: "fn main() {}\n".to_string(),
        })
        .unwrap();
    assert!(
        !verdict.passed_checks.is_empty() || !verdict.violations.is_empty(),
        "rule must have been evaluated for absolute path, got: {verdict:?}"
    );
}
```

- [ ] **Step 4: Verify the runner test fails** for the same reason.

Run: `cargo test -p hector-core --test runner_absolute_path -- --nocapture`
Expected: FAIL — `passed_checks` and `violations` both empty (rule never matched).

- [ ] **Step 5: Implement** — canonicalize the input path relative to `config_dir` before scope match. In `runner.rs:120-253` add a helper near the top of `check`:

```rust
fn relativize<'a>(path: &'a std::path::Path, root: &std::path::Path) -> std::path::PathBuf {
    use std::path::PathBuf;
    // Resolve both sides through canonicalize where possible; fall back to as-is
    // when the file is missing (diff mode may reference paths the FS doesn't have).
    let canon_path = path.canonicalize().unwrap_or_else(|_| PathBuf::from(path));
    let canon_root = root.canonicalize().unwrap_or_else(|_| PathBuf::from(root));
    canon_path
        .strip_prefix(&canon_root)
        .map(PathBuf::from)
        .unwrap_or(canon_path)
}
```

Then change the loop body in `runner.rs` (currently `if !matcher.matches(&path)`) to use the relativized path for matching while keeping the original for reporting:

```rust
let match_path = relativize(&path, &self.config_dir);
for (rule_id, rule) in &self.config.rules {
    let matcher = crate::config::scope::ScopeMatcher::new(&rule.scope)
        .expect("scope validated at load");
    if !matcher.matches(&match_path) {
        continue;
    }
    // ... rest unchanged; violations still use `path.display()` for `file`
```

- [ ] **Step 6: Verify both tests pass**

Run: `cargo test -p hector-core --test scope && cargo test -p hector-core --test runner_absolute_path`
Expected: PASS.

- [ ] **Step 7: Verify nothing else broke**

Run: `cargo test --workspace && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 8: Commit**

```bash
git add crates/hector-core/src/runner.rs crates/hector-core/src/config/scope.rs \
        crates/hector-core/tests/scope.rs crates/hector-core/tests/runner_absolute_path.rs
git commit -m "$(cat <<'EOF'
fix(scope): match relative globs against absolute input paths (P0-1)

Adapters (Claude Code hook, opencode plugin) pass absolute paths to
`hector check`; relative globs like `crates/*/src/**/*.rs` matched nothing,
producing silent Pass for every check in production.

Canonicalize the input path relative to config_dir before scope matching
while preserving the original for violation reporting.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 1 — RCE Chain Mitigation (P0-2, P0-3, P0-4)

Trust gate bypass via `extends:`, shell injection via `{file}`, and path traversal in diff parser. Each is exploitable alone; together they're unauthenticated RCE. The three tasks below touch disjoint files, so a coordinator may dispatch them as parallel subagents.

### Task 1.1: Verify trust on every config in the `extends:` chain [parallel]

**Files:**
- Modify: `crates/hector-core/src/runner.rs:58-71`, `crates/hector-core/src/config/extends.rs:13-33`
- Test: `crates/hector-core/tests/extends.rs`

- [ ] **Step 1: Write the failing test** in `tests/extends.rs`:

```rust
#[test]
fn extends_chain_rejects_untrusted_parent() {
    use hector_core::runner::HectorEngine;
    let tmp = tempfile::tempdir().unwrap();
    // Parent has a script rule but NO trust block — would execute arbitrary code.
    let parent = "schema_version: 2\nrules:\n  exfil:\n    description: bad\n    engine: script\n    scope: [\"**/*\"]\n    severity: error\n    script: \"touch /tmp/PWNED\"\n";
    std::fs::write(tmp.path().join("parent.yml"), parent).unwrap();
    let child_raw = "schema_version: 2\nextends: [\"parent.yml\"]\nrules: {}\n";
    let trusted = hector_core::trust::write_trust_block(child_raw).unwrap();
    let child = tmp.path().join("child.yml");
    std::fs::write(&child, trusted).unwrap();

    let err = HectorEngine::load(&child).expect_err("must reject untrusted parent");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("trust") || msg.contains("fingerprint"),
        "error should reference trust; got: {msg}"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p hector-core --test extends extends_chain_rejects_untrusted_parent -- --nocapture`
Expected: FAIL — engine loads cleanly because parent's trust isn't verified.

- [ ] **Step 3: Implement** — call `trust::verify` for each parent before parsing. In `config/extends.rs`, change `resolve_inner`:

```rust
fn resolve_inner(path: &Path, seen: &mut HashSet<PathBuf>) -> Result<Config> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", path.display()))?;
    if !seen.insert(canonical.clone()) {
        return Err(anyhow!("extends cycle detected at {}", canonical.display()));
    }
    let content = std::fs::read_to_string(&canonical)
        .with_context(|| format!("reading {}", canonical.display()))?;
    // CHANGED: every parent in the chain must carry its own trust block.
    crate::trust::verify(&content)
        .with_context(|| format!("trust verify for {}", canonical.display()))?;
    let mut cfg = parse_str(&content)?;
    // ... rest unchanged
}
```

The root config is already verified by the runner — to avoid double-verification (and to keep tests simple), the runner can keep verifying its own input and `resolve_inner` only verifies *inherited* configs. The cleanest split: in `runner.rs:58-71`, remove the explicit `trust::verify(&raw)` and let `parse_file_with_extends` verify every config including the root. Update `extends.rs` to *always* verify.

- [ ] **Step 4: Run all extends tests**

Run: `cargo test -p hector-core --test extends`
Expected: the new test passes; existing tests pass (they all carry trust blocks already).

- [ ] **Step 5: Commit**

```bash
git add crates/hector-core/src/config/extends.rs crates/hector-core/src/runner.rs \
        crates/hector-core/tests/extends.rs
git commit -m "$(cat <<'EOF'
fix(trust): verify every config in extends chain (P0-2)

extends: previously only verified the root config; an attacker who could
write to any parent config could plant arbitrary script: rules and the
signed child still loaded cleanly.

Verify trust at every node in the extends DFS.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.2: Eliminate shell injection in `{file}` substitution [parallel]

**Files:**
- Modify: `crates/hector-core/src/engine/script.rs:33-74`, `crates/hector-core/src/engine/capability.rs:14-87`
- Test: `crates/hector-core/tests/e2e_script_rules.rs`, `crates/hector-core/tests/script_engine.rs`

- [ ] **Step 1: Write the failing test** in `tests/script_engine.rs`:

```rust
#[test]
fn script_engine_quotes_file_path_with_shell_metacharacters() {
    use hector_core::config::{EngineKind, Rule, Severity};
    use hector_core::engine::script::run_script_rule;
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    // Filename that, if interpolated unquoted, would `touch PWNED`.
    let evil_name = "a; touch PWNED; b.txt";
    let evil = cwd.join(evil_name);
    std::fs::write(&evil, "hi").unwrap();
    let rule = Rule {
        description: "echo only".into(),
        engine: EngineKind::Script,
        scope: vec!["**/*".into()],
        severity: Severity::Warning,
        // The script is expected to receive the path as $1, not as text.
        script: Some("ls -- {file} >/dev/null".into()),
        pattern: None,
        language: None,
        context: None,
        capabilities: None,
        fix_hint: None,
    };
    let _ = run_script_rule("evil", &rule, &evil, "", cwd);
    assert!(
        !cwd.join("PWNED").exists(),
        "shell injection succeeded — PWNED marker was created"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p hector-core --test script_engine script_engine_quotes_file_path_with_shell_metacharacters -- --nocapture`
Expected: FAIL — `PWNED` file exists.

- [ ] **Step 3: Implement** — pass the file path as an argv parameter, never substituted into the command text.

In `engine/script.rs:33-49`, replace `{file}` substitution with a sentinel that becomes a positional argument:

```rust
fn run_script_rule_internal(
    rule_id: &str,
    rule: &Rule,
    file: &Path,
    _diff: &str,
    cwd: &Path,
) -> Result<Option<Violation>> {
    let script = rule
        .script
        .as_ref()
        .ok_or_else(|| anyhow!("rule {rule_id} is engine: script but has no `script:` field"))?;
    // `{file}` becomes the shell parameter `"$HECTOR_FILE"`. The actual path is
    // exported in the child environment, never spliced into the script text,
    // so shell metacharacters in the filename can't escape.
    let substituted = script.replace("{file}", "\"$HECTOR_FILE\"");
    let caps = rule.capabilities.clone().unwrap_or_default();
    let outcome = crate::engine::capability::run_with_capabilities_env(
        &substituted,
        cwd,
        &caps,
        &[("HECTOR_FILE", &file.display().to_string())],
    )?;
    // ... rest unchanged
}
```

In `engine/capability.rs`, introduce `run_with_capabilities_env` that wraps the existing one with `.env(name, value)` on the `Command`:

```rust
pub fn run_with_capabilities_env(
    cmd: &str,
    cwd: &Path,
    caps: &Capabilities,
    env: &[(&str, &str)],
) -> Result<ExecOutcome> {
    // Identical to run_with_capabilities but applies envs before spawning.
    // ... implementation that calls `.env(k, v)` on the Command and otherwise
    //     mirrors the existing linux/non-linux split
}
```

(Or: add an optional `env: &[(&str, &str)]` parameter to `run_with_capabilities` directly. Either is fine; the new function keeps the existing public API stable.)

- [ ] **Step 4: Verify the injection test passes**

Run: `cargo test -p hector-core --test script_engine script_engine_quotes_file_path_with_shell_metacharacters`
Expected: PASS.

- [ ] **Step 5: Run full e2e to verify legit rules still work**

Run: `cargo test -p hector-core --test e2e_script_rules`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/hector-core/src/engine/script.rs crates/hector-core/src/engine/capability.rs \
        crates/hector-core/tests/script_engine.rs
git commit -m "$(cat <<'EOF'
fix(script): pass file path via env, not text substitution (P0-3)

{file} previously expanded into the shell command before `sh -c`, so a
filename with metacharacters (`a; touch x; b.py`) would inject arbitrary
shell. Replace {file} with "$HECTOR_FILE" and pass the path via the child
environment so shell parsing can't escape it.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.3: Reject path traversal in unified diffs [parallel]

**Files:**
- Modify: `crates/hector-core/src/diff/parser.rs:11-55`
- Test: `crates/hector-core/tests/diff_parse.rs`

- [ ] **Step 1: Write the failing test** in `tests/diff_parse.rs`:

```rust
#[test]
fn parse_unified_rejects_path_traversal() {
    let diff = "--- a/foo\n+++ b/../../../etc/passwd\n@@ -0,0 +1 @@\n+x\n";
    let err = hector_core::diff::parser::parse_unified(diff)
        .expect_err("path traversal must be rejected");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("traversal") || msg.contains("absolute") || msg.contains(".."),
        "error should mention traversal; got: {msg}"
    );
}

#[test]
fn parse_unified_rejects_absolute_path() {
    let diff = "--- a/foo\n+++ b//etc/passwd\n@@ -0,0 +1 @@\n+x\n";
    assert!(hector_core::diff::parser::parse_unified(diff).is_err());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p hector-core --test diff_parse parse_unified_rejects_path_traversal -- --nocapture`
Expected: FAIL — parser accepts the traversal.

- [ ] **Step 3: Implement** in `diff/parser.rs:17-24`:

```rust
if let Some(path) = raw.strip_prefix("+++ b/") {
    let path = path.trim_end_matches('\r');       // also fixes P2-10
    if path.starts_with('/') {
        return Err(anyhow!("diff contains absolute path: {path}"));
    }
    let pb = PathBuf::from(path);
    if pb.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(anyhow!("diff contains path traversal: {path}"));
    }
    if let Some(f) = current.take() {
        files.push(f);
    }
    current = Some(ChangedFile {
        path: pb,
        added_lines: Vec::new(),
    });
}
```

- [ ] **Step 4: Add a CRLF test** in the same file (this also closes P2-10):

```rust
#[test]
fn parse_unified_trims_crlf_from_path() {
    let diff = "--- a/foo\r\n+++ b/myfile.py\r\n@@ -0,0 +1 @@\n+x\n";
    let files = hector_core::diff::parser::parse_unified(diff).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::PathBuf::from("myfile.py"),
        "trailing \\r must be stripped");
}
```

- [ ] **Step 5: Verify all tests pass**

Run: `cargo test -p hector-core --test diff_parse`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/hector-core/src/diff/parser.rs crates/hector-core/tests/diff_parse.rs
git commit -m "$(cat <<'EOF'
fix(diff): reject path traversal and absolute paths in `+++ b/` (P0-4, P2-10)

The diff parser fed `path` straight into `PathBuf` without checking for `..`
or a leading `/`. A malicious diff with `+++ b/../../../etc/passwd` could
exfiltrate via semantic context-read or — chained with the shell-injection
fix that landed in P0-3 — a script rule. Also trim trailing `\r` so CRLF
diffs no longer silently mis-match.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — Diff Mode Hardening (P0-5, P0-6, P0-7)

All three findings have one root cause: in `runner.rs:123-126` the diff branch passes `content = String::new()`, which (a) makes AST guarantee-fail with "requires file content", (b) makes `DisableMap::from_source(&content)` empty, and (c) `commands/check.rs` only feeds the first file from the diff. Fix as a coordinated refactor.

### Task 2.1: Read the file from disk in diff mode, iterate all files

**Files:**
- Modify: `crates/hector-core/src/runner.rs:120-253`
- Modify: `crates/hector-cli/src/commands/check.rs:35-58`
- Test: `crates/hector-core/tests/runner_diff.rs`, `crates/hector-cli/tests/cli_check.rs`

- [ ] **Step 1: Write the AST-in-diff-mode failing test** in `tests/runner_diff.rs`:

```rust
#[test]
fn ast_rule_runs_in_diff_mode_when_file_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let target = root.join("src/foo.rs");
    std::fs::write(&target, "fn main() { let _ = foo.unwrap(); }\n").unwrap();
    let cfg = "schema_version: 2\nrules:\n  no-unwrap:\n    description: x\n    engine: ast\n    language: rust\n    scope: [\"src/**/*.rs\"]\n    severity: warning\n    pattern: $E.unwrap()\n";
    let trusted = hector_core::trust::write_trust_block(cfg).unwrap();
    let cfg_path = root.join(".hector.yml");
    std::fs::write(&cfg_path, trusted).unwrap();
    let engine = hector_core::runner::HectorEngine::load(&cfg_path).unwrap();
    let diff = format!(
        "--- a/src/foo.rs\n+++ b/src/foo.rs\n@@ -1 +1 @@\n-fn main() {{}}\n+fn main() {{ let _ = foo.unwrap(); }}\n"
    );
    let v = engine
        .check(hector_core::runner::CheckInput::Diff {
            file: target.clone(),
            unified_diff: diff,
        })
        .unwrap();
    // Pre-fix: this produces a Block with an `__internal` error about file content.
    // Post-fix: the rule runs against on-disk content and produces a real Warn.
    assert!(
        v.violations.iter().any(|x| x.rule_id == "no-unwrap"),
        "ast rule must fire in diff mode; got {v:?}"
    );
    assert!(
        v.violations.iter().all(|x| !x.rule_id.ends_with("__internal")),
        "no internal-error violations expected; got {v:?}"
    );
}
```

- [ ] **Step 2: Write the disable-in-diff-mode failing test** in `tests/runner_diff.rs`:

```rust
#[test]
fn hector_disable_directive_applies_in_diff_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let target = root.join("src/foo.rs");
    std::fs::write(
        &target,
        "fn main() { let _ = foo.unwrap(); } // hector-disable: no-unwrap\n",
    )
    .unwrap();
    let cfg = "schema_version: 2\nrules:\n  no-unwrap:\n    description: x\n    engine: ast\n    language: rust\n    scope: [\"src/**/*.rs\"]\n    severity: error\n    pattern: $E.unwrap()\n";
    let trusted = hector_core::trust::write_trust_block(cfg).unwrap();
    let cfg_path = root.join(".hector.yml");
    std::fs::write(&cfg_path, trusted).unwrap();
    let engine = hector_core::runner::HectorEngine::load(&cfg_path).unwrap();
    let diff = "--- a/src/foo.rs\n+++ b/src/foo.rs\n@@ -1 +1 @@\n-x\n+fn main() { let _ = foo.unwrap(); } // hector-disable: no-unwrap\n";
    let v = engine
        .check(hector_core::runner::CheckInput::Diff {
            file: target,
            unified_diff: diff.to_string(),
        })
        .unwrap();
    assert!(
        !v.violations.iter().any(|x| x.rule_id == "no-unwrap"),
        "hector-disable on the same line must silence the rule, got {v:?}"
    );
}
```

- [ ] **Step 3: Write the multi-file failing test** in `crates/hector-cli/tests/cli_check.rs`:

```rust
#[test]
fn cli_check_diff_processes_every_changed_file() {
    use assert_cmd::Command;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src/b.rs"), "fn b() { panic!(); }\n").unwrap();
    let cfg = "schema_version: 2\nrules:\n  no-panic:\n    description: x\n    engine: ast\n    language: rust\n    scope: [\"src/**/*.rs\"]\n    severity: error\n    pattern: panic!($$$)\n";
    let trusted = hector_core::trust::write_trust_block(cfg).unwrap();
    let cfg_path = root.join(".hector.yml");
    std::fs::write(&cfg_path, trusted).unwrap();
    let diff = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-x\n+fn a() {}\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1 @@\n-x\n+fn b() { panic!(); }\n";
    let diff_path = root.join("multi.diff");
    std::fs::write(&diff_path, diff).unwrap();
    let out = Command::cargo_bin("hector").unwrap()
        .args(["check", "--diff", diff_path.to_str().unwrap(),
               "--config", cfg_path.to_str().unwrap(),
               "--format", "json"])
        .current_dir(root)
        .output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"rule_id\": \"no-panic\""), "violation must surface for src/b.rs: {stdout}");
    // Exit 2 because the violation is Error severity → Block.
    assert_eq!(out.status.code(), Some(2));
}
```

- [ ] **Step 4: Verify all three fail**

Run: `cargo test -p hector-core --test runner_diff ast_rule_runs_in_diff_mode_when_file_on_disk hector_disable_directive_applies_in_diff_mode -- --nocapture && cargo test -p hector-cli --test cli_check cli_check_diff_processes_every_changed_file -- --nocapture`
Expected: all three FAIL.

- [ ] **Step 5: Refactor `runner.rs:120-253`** to read content from disk in diff mode. Change the `check` head from:

```rust
let (path, content, diff) = match input {
    CheckInput::File { path, content } => (path, content, String::new()),
    CheckInput::Diff { file, unified_diff } => (file, String::new(), unified_diff),
};
```

to:

```rust
let (path, content, diff) = match input {
    CheckInput::File { path, content } => (path, content, String::new()),
    CheckInput::Diff { file, unified_diff } => {
        // Read the post-edit file from disk so AST rules, disable directives,
        // and any file-content-based engine see real content. In agent flows
        // this is the file *after* the agent's edit landed.
        let content = std::fs::read_to_string(&file).unwrap_or_default();
        (file, content, unified_diff)
    }
};
```

This single change resolves P0-5 (AST has content), P0-7 (`DisableMap::from_source(&content)` now sees real lines), and AST stops landing in `__internal` error. The `unwrap_or_default()` keeps the runner resilient if the diff references a file that's been deleted: AST then simply skips with `Ok(None)` (its `content.ok_or_else` returns to the runner, which today produces Internal — we'll fix that downstream of P1-1, see Phase 5).

- [ ] **Step 6: Refactor `commands/check.rs:40-48`** to iterate all files from the parsed diff. Replace the single-file path with:

```rust
(None, Some(d)) => {
    let unified_diff = std::fs::read_to_string(&d)?;
    let changed = hector_core::diff::parser::parse_unified(&unified_diff)?;
    if changed.is_empty() {
        eprintln!("ERROR: no changed files in diff");
        return Ok(1);
    }
    let mut aggregated_violations = Vec::new();
    let mut aggregated_passed = Vec::new();
    let mut elapsed_ms = 0u64;
    for f in changed {
        let per_file_diff = build_single_file_diff(&unified_diff, &f.path);
        let v = engine.check(CheckInput::Diff {
            file: f.path,
            unified_diff: per_file_diff,
        })?;
        elapsed_ms += v.elapsed_ms;
        aggregated_violations.extend(v.violations);
        aggregated_passed.extend(v.passed_checks);
    }
    let verdict = hector_core::verdict::Verdict::from_violations(
        aggregated_violations,
        aggregated_passed,
        elapsed_ms,
    );
    emit(&verdict, format)?;
    return Ok(exit_code(&verdict));
}
```

`build_single_file_diff` is a new helper that slices the unified diff to just the hunks for one path:

```rust
fn build_single_file_diff(full: &str, file: &std::path::Path) -> String {
    let needle = format!("+++ b/{}", file.display());
    let mut out = String::new();
    let mut keep = false;
    for line in full.split_inclusive('\n') {
        if line.starts_with("+++ b/") {
            keep = line.starts_with(&needle);
        } else if line.starts_with("--- ") {
            // gate state will be set by the matching `+++ b/` next iteration
            keep = false;
        }
        if keep || line.starts_with("--- ") {
            out.push_str(line);
        }
    }
    out
}
```

Remove the now-unused `first_file_in_diff` helper.

- [ ] **Step 7: Verify all three tests pass**

Run: `cargo test -p hector-core --test runner_diff && cargo test -p hector-cli --test cli_check`
Expected: PASS.

- [ ] **Step 8: Verify nothing else broke**

Run: `cargo test --workspace && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 9: Commit**

```bash
git add crates/hector-core/src/runner.rs crates/hector-cli/src/commands/check.rs \
        crates/hector-core/tests/runner_diff.rs crates/hector-cli/tests/cli_check.rs
git commit -m "$(cat <<'EOF'
fix(diff-mode): read file on disk; iterate every changed file (P0-5, P0-6, P0-7)

Diff mode previously passed empty `content` to the runner, which made:
- AST rules guarantee-fail with "requires file content" (P0-5),
- DisableMap empty so hector-disable directives never applied (P0-7),
- only the first file in a multi-file diff get checked (P0-6).

Read the on-disk post-edit content in the runner's diff branch, and have
the CLI iterate every changed file from `parse_unified` and aggregate
verdicts.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — Capability Sandbox Honesty (P0-8, P0-9)

The sandbox today EPERMs on every unprivileged Linux runner (P0-8) and the writes policy is silently a no-op (P0-9). Pick one path:

**A. Implement properly** — add `CLONE_NEWUSER` with UID/GID maps, implement the mount remount. High effort, requires careful UID-mapping work, but delivers on the docs.
**B. Document as advisory** — admit on every platform that capability enforcement is best-effort, remove `writes:` variants from the schema (or accept them as no-ops with a deprecation warning), update `docs/security.md` and `CLAUDE.md`.

Recommendation: do **B** for 0.1, lift to **A** in a follow-up. Trust is the security boundary at 0.1; capabilities were always accident-protection. Pretending they're more is the problem.

### Task 3.1: Make capability sandbox work for unprivileged users [serial within phase]

**Files:**
- Modify: `crates/hector-core/src/engine/capability.rs:25-65`
- Test: `crates/hector-core/tests/capability.rs`

- [ ] **Step 1: Write the failing test** in `tests/capability.rs`. Guard with `cfg(target_os = "linux")` and `cfg(not(uid_zero))`:

```rust
#[cfg(target_os = "linux")]
#[test]
fn capability_run_succeeds_for_unprivileged_user() {
    use hector_core::config::{Capabilities, WritesPolicy};
    use hector_core::engine::capability::run_with_capabilities;
    // Pre-fix: unshare(CLONE_NEWNET) without CLONE_NEWUSER returns EPERM
    // for unprivileged callers, so `Command::output()` errors.
    let caps = Capabilities { network: false, writes: WritesPolicy::CwdOnly };
    let out = run_with_capabilities("echo ok", std::path::Path::new("/tmp"), &caps)
        .expect("must run without privilege");
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("ok"));
}
```

- [ ] **Step 2: Run to confirm it fails on an unprivileged runner**

Run (Linux non-root): `cargo test -p hector-core --test capability capability_run_succeeds_for_unprivileged_user`
Expected: FAIL — `unshare: EPERM`.

- [ ] **Step 3: Implement** the documentation-first path:

In `engine/capability.rs`, replace the existing `run_linux` with a guarded version that detects EPERM and falls back, with a one-time stderr warning:

```rust
#[cfg(target_os = "linux")]
fn run_linux(cmd: &str, cwd: &Path, caps: &Capabilities) -> Result<ExecOutcome> {
    use nix::sched::{unshare, CloneFlags};
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);

    let mut flags = CloneFlags::empty();
    if !caps.network { flags.insert(CloneFlags::CLONE_NEWNET); }
    if matches!(caps.writes, WritesPolicy::None | WritesPolicy::CwdOnly) {
        flags.insert(CloneFlags::CLONE_NEWNS);
    }

    // Probe whether unshare with the requested flags would succeed. If not,
    // fall back to best-effort with a one-time warning. (CLONE_NEWUSER would
    // make this work unprivileged, but proper UID/GID mapping is out of scope
    // for 0.1 — see plans/2026-05-12-bug-audit-remediation.md Phase 3.)
    if !flags.is_empty() {
        let probe = unsafe { libc::unshare(flags.bits()) };
        if probe != 0 {
            let err = std::io::Error::last_os_error();
            if !WARNED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "hector: capability sandbox unavailable for unprivileged user ({err}); \
                     running command without isolation. See docs/security.md."
                );
            }
            return run_best_effort(cmd, cwd, caps);
        }
        // probe succeeded — we're already inside the new namespace. Spawn child
        // and inherit it (don't unshare again in pre_exec).
    }
    let output = Command::new("sh").arg("-c").arg(cmd).current_dir(cwd).output().context("running command")?;
    Ok(ExecOutcome {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}
```

Note the change: we unshare in the parent process (after probing) rather than in `pre_exec`. The current `pre_exec` version errors *inside* the spawn and surfaces as a confusing "running command" error. The probe-and-warn path is more diagnostic.

Add the `libc` dependency to `hector-core/Cargo.toml`. Put it under the top-level `[dependencies]` (not the Linux target block) because telemetry's `flock` in Phase 7 also uses it on macOS:

```toml
[dependencies]
# ... existing
libc = "0.2"
```

- [ ] **Step 4: Run the test to confirm it now passes**

Expected: PASS — on Linux non-root it warns once and falls back; on root it unshares.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-core/src/engine/capability.rs crates/hector-core/tests/capability.rs \
        crates/hector-core/Cargo.toml
git commit -m "$(cat <<'EOF'
fix(capability): graceful fallback when unprivileged (P0-8)

unshare(CLONE_NEWNET|CLONE_NEWNS) returned EPERM for unprivileged users,
which made `Command::output()` error and every script rule produce an
`__internal` Block. Probe in the parent, fall back to best-effort with a
one-time stderr warning when privilege is missing.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3.2: Stop advertising `writes:` policies that aren't enforced

**Files:**
- Modify: `crates/hector-core/src/engine/capability.rs:60-65` (delete dead stub)
- Modify: `docs/security.md`, `CLAUDE.md`
- Optional schema change: `crates/hector-core/src/config/types.rs:98-105`

- [ ] **Step 1: Document the limitation honestly.** Edit `docs/security.md` (or create if missing) with a section "Writes policy enforcement (0.1):"

```
The schema accepts `writes: none | cwd_only | tmp | unrestricted` but
**0.1 does not enforce any of them**. All four behave identically:
the spawned process can write anywhere it has POSIX permission to.

Why: enforcement requires CAP_SYS_ADMIN inside a user namespace plus
careful bind-mount remounts; the work is tracked for 0.2. Until then,
treat `writes:` as advisory documentation, not as a control.

If you need write isolation today, run hector inside an OS-level
sandbox (e.g., a container, a fresh user, or `bwrap`).
```

- [ ] **Step 2: Update `CLAUDE.md`.** Find the line "writes policies use `CLONE_NEWNS`" and replace with:

```
**Capability sandbox** (`engine/capability.rs`) is **Linux-strict for network, advisory for writes**.
On Linux, `network: false` unshare's the net namespace when privileged
(falls back to best-effort with a warning when not). The writes policy
is currently a no-op pending CAP_SYS_ADMIN-via-CLONE_NEWUSER work in 0.2.
On macOS, all capability constraints are advisory and the command runs
unrestricted.
```

- [ ] **Step 3: Delete the dead `apply_mount_policy` stub** (`engine/capability.rs:59-65`) and the call site inside `pre_exec`. Keep `CLONE_NEWNS` only when writes is None/CwdOnly so the namespace flag isn't silently set when nothing uses it — or, better, drop the flag entirely for now to match the docs.

- [ ] **Step 4: Verify the existing capability tests still pass**

Run: `cargo test -p hector-core --test capability`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-core/src/engine/capability.rs docs/security.md CLAUDE.md
git commit -m "$(cat <<'EOF'
docs(capability): say plainly that writes policy is not enforced in 0.1 (P0-9)

The mount-policy stub did nothing, but CLAUDE.md and the schema implied
otherwise. Delete the stub, document the actual enforcement boundary,
and reword CLAUDE.md to match reality. Schema variants stay (no breaking
change); they become real in 0.2 when CLONE_NEWUSER lands.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4 — Baseline UX (P0-10)

`hector baseline` walks every file with `walkdir` and reads each into memory. On Hector's own repo with `target/` populated, this OOMs or takes minutes. Single task.

### Task 4.1: gitignore-aware, parallel baseline walk

**Files:**
- Modify: `crates/hector-cli/src/commands/baseline.rs:6-49`
- Modify: `crates/hector-cli/Cargo.toml` (add `ignore = "0.4"`, `rayon = "1"`)
- Test: `crates/hector-cli/tests/cli_baseline.rs`

- [ ] **Step 1: Write the failing test** in `tests/cli_baseline.rs`:

```rust
#[test]
fn baseline_skips_gitignored_and_target_dirs() {
    use assert_cmd::Command;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Real source.
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/foo.rs"), "fn main() { let _ = x.unwrap(); }\n").unwrap();
    // .gitignore'd build artifact.
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::write(root.join("target/debug/junk.rs"), "fn main() { let _ = x.unwrap(); }\n").unwrap();
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    // Hector config.
    let cfg = "schema_version: 2\nrules:\n  no-unwrap:\n    description: x\n    engine: ast\n    language: rust\n    scope: [\"**/*.rs\"]\n    severity: warning\n    pattern: $E.unwrap()\n";
    let trusted = hector_core::trust::write_trust_block(cfg).unwrap();
    let cfg_path = root.join(".hector.yml");
    std::fs::write(&cfg_path, trusted).unwrap();
    let out = Command::cargo_bin("hector").unwrap()
        .args(["baseline", "--config", cfg_path.to_str().unwrap()])
        .current_dir(root)
        .output().unwrap();
    assert!(out.status.success(), "{:?}", String::from_utf8_lossy(&out.stderr));
    let baseline: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join(".hector/baseline.json")).unwrap()).unwrap();
    let fps = baseline["fingerprints"].as_array().unwrap();
    let printed: Vec<String> = fps.iter().map(|v| v.as_str().unwrap().to_string()).collect();
    assert!(printed.iter().any(|f| f.contains("src/foo.rs")), "src/foo.rs must be baselined: {printed:?}");
    assert!(
        !printed.iter().any(|f| f.contains("target/")),
        ".gitignored target/ must be skipped: {printed:?}"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p hector-cli --test cli_baseline baseline_skips_gitignored_and_target_dirs -- --nocapture`
Expected: FAIL — `target/` entries appear in the baseline.

- [ ] **Step 3: Add deps** to `crates/hector-cli/Cargo.toml`:

```toml
[dependencies]
# ... existing
ignore = "0.4"
rayon = "1"
```

- [ ] **Step 4: Rewrite `commands/baseline.rs`** to use the `ignore` walker (gitignore-aware) and parallelize via `rayon`:

```rust
use anyhow::Result;
use hector_core::baseline::Baseline;
use hector_core::runner::{CheckInput, HectorEngine};
use ignore::WalkBuilder;
use rayon::prelude::*;
use std::path::Path;
use std::sync::Mutex;

pub fn run(config: &Path, scan_glob: Option<String>) -> Result<i32> {
    let engine = HectorEngine::load(config)?;
    let dir = config
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let baseline_path = dir.join(".hector/baseline.json");
    let baseline = Mutex::new(Baseline::load(&baseline_path)?);

    let pattern = scan_glob.unwrap_or_else(|| "**/*".to_string());
    let glob = globset::Glob::new(&pattern)?.compile_matcher();

    // Collect first so we can parallelize. The `ignore` walker honors
    // .gitignore, .ignore, and global excludes; it also skips hidden dirs
    // and `target/` automatically when .gitignore lists them.
    let paths: Vec<_> = WalkBuilder::new(dir)
        .standard_filters(true)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
        .filter(|p| {
            let rel = p.strip_prefix(dir).unwrap_or(p);
            glob.is_match(rel)
        })
        .collect();

    paths.par_iter().for_each(|path| {
        let Ok(content) = std::fs::read_to_string(path) else { return };
        let Ok(verdict) = engine.check(CheckInput::File {
            path: path.clone(),
            content,
        }) else { return };
        let mut bl = baseline.lock().unwrap();
        for v in verdict.violations { bl.add(&v); }
    });

    let baseline = baseline.into_inner().unwrap();
    baseline.save(&baseline_path)?;
    println!(
        "baseline written: {} ({} entries)",
        baseline_path.display(),
        baseline.fingerprints.len()
    );
    Ok(0)
}
```

- [ ] **Step 5: Verify the new test passes**

Run: `cargo test -p hector-cli --test cli_baseline`
Expected: PASS.

- [ ] **Step 6: Verify the whole suite passes**

Run: `cargo test --workspace && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add crates/hector-cli/src/commands/baseline.rs crates/hector-cli/Cargo.toml \
        crates/hector-cli/tests/cli_baseline.rs Cargo.toml
git commit -m "$(cat <<'EOF'
fix(baseline): gitignore-aware parallel walk (P0-10)

Default scan walked every file including target/ and node_modules/ and
ran each rule serially. Switched to the `ignore` crate (honors
.gitignore, .ignore, and global excludes) and parallelized the per-file
check via rayon. The slow-and-OOMs path no longer exists.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5 — Verdict Shape Settle (P1-1, P1-3)

Locked-but-unstable until 0.3. After freeze, breaking shape changes cost a `SCHEMA_VERSION` bump. Land these now.

### Task 5.1: Split `Engine::Trust` into `Engine::Trust` and `Engine::Internal` [parallel]

**Files:**
- Modify: `crates/hector-core/src/verdict.rs:3,43-51`
- Modify: `crates/hector-core/src/runner.rs:222,278`
- Test: `crates/hector-core/tests/verdict_snapshot.rs`

- [ ] **Step 1: Add a snapshot test asserting both variants** in `tests/verdict_snapshot.rs`:

```rust
#[test]
fn engine_enum_separates_trust_from_internal() {
    use hector_core::verdict::Engine;
    let trust = serde_json::to_string(&Engine::Trust).unwrap();
    let internal = serde_json::to_string(&Engine::Internal).unwrap();
    assert_eq!(trust, "\"trust\"");
    assert_eq!(internal, "\"internal\"");
}
```

- [ ] **Step 2: Verify it fails**

Run: `cargo test -p hector-core --test verdict_snapshot engine_enum_separates_trust_from_internal`
Expected: FAIL — `Engine::Internal` doesn't exist.

- [ ] **Step 3: Edit `verdict.rs:3`** — bump schema version since we're changing shape now (before freeze):

```rust
pub const SCHEMA_VERSION: u32 = 2;
```

Edit `verdict.rs:43-51`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Script,
    Ast,
    Semantic,
    Session,
    Trust,
    Internal,
}
```

- [ ] **Step 4: Update `runner.rs:222` and `runner.rs:278`** to use `Engine::Internal` for engine-error violations (LLM down, AST refused, script spawn failure). Reserve `Engine::Trust` for true trust-gate failures — those today fail at `load_with`, never reach `check`, so the only remaining use is theoretical and we should add a test that asserts trust failures *don't* land in a Violation:

```rust
// runner.rs:222 (Err arm of the per-rule match in `check`)
Err(e) => {
    violations.push(Violation {
        rule_id: format!("{rule_id}__internal"),
        severity: crate::verdict::Severity::Error,
        engine: crate::verdict::Engine::Internal, // was Engine::Trust
        // ... rest unchanged
    });
}
```

Same change at `runner.rs:278`.

- [ ] **Step 5: Refresh snapshots if any reference the old shape**

Run: `cargo insta test` then `cargo insta review` to accept the schema_version bump.

- [ ] **Step 6: Verify the workspace**

Run: `cargo test --workspace && cargo clippy --all-targets -- -D warnings`
Expected: green (after insta review).

- [ ] **Step 7: Commit**

```bash
git add crates/hector-core/src/verdict.rs crates/hector-core/src/runner.rs \
        crates/hector-core/tests/verdict_snapshot.rs crates/hector-core/tests/snapshots
git commit -m "$(cat <<'EOF'
verdict(shape): add Engine::Internal; bump SCHEMA_VERSION to 2 (P1-1)

Engine::Trust was overloaded: trust-gate failures and engine-internal
errors (LLM down, AST refused diff, script spawn failure) all landed
there. Split into Engine::Internal for runtime errors; reserve
Engine::Trust for verifiable trust-block failures. Bumping
SCHEMA_VERSION now is cheap; doing it after 0.3 freeze is a breaking
change for every CI/editor consumer.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 5.2: Populate (or remove) `Violation.column` and `Violation.context` [parallel]

**Files:**
- Modify: `crates/hector-core/src/engine/ast.rs:60-69` (column has data from `start_pos()`)
- Modify: `crates/hector-core/src/engine/{script,semantic,session,ast}.rs` (context for AST is N surrounding lines)
- Test: `crates/hector-core/tests/ast_engine.rs`, `crates/hector-core/tests/verdict_snapshot.rs`

Decision required: populate or remove? Recommendation: **populate** for AST (`column` from `start_pos().column()`, `context` as ±3 lines around the match). For script/semantic/session: leave `column = None`; populate `context` for semantic by including the surrounding hunk; for script and session leave both `None` because the engine doesn't have positional info. This keeps the shape useful where data exists and explicitly None where it doesn't.

- [ ] **Step 1: Write the failing test** in `tests/ast_engine.rs`:

```rust
#[test]
fn ast_violation_populates_column_and_context() {
    let rule = hector_core::config::Rule {
        description: "x".into(),
        engine: hector_core::config::EngineKind::Ast,
        scope: vec!["**/*.rs".into()],
        severity: hector_core::config::Severity::Warning,
        script: None,
        pattern: Some("$E.unwrap()".into()),
        language: Some("rust".into()),
        context: None,
        capabilities: None,
        fix_hint: None,
    };
    let ctx = hector_core::engine::RuleContext {
        rule_id: "no-unwrap",
        rule: &rule,
        file: std::path::Path::new("test.rs"),
        content: Some("fn a() {\n    foo();\n    bar.unwrap();\n    baz();\n}\n"),
        diff: None,
        cwd: std::path::Path::new("."),
        llm: None,
    };
    use hector_core::engine::RuleEngine;
    let v = hector_core::engine::ast::AstEngine.run(&ctx).unwrap().unwrap();
    assert!(v.column.is_some(), "column must be populated for ast");
    let ctxstr = v.context.expect("context must be populated for ast");
    assert!(ctxstr.contains("foo();") && ctxstr.contains("bar.unwrap();") && ctxstr.contains("baz();"),
        "context should include surrounding ±N lines: {ctxstr}");
}
```

- [ ] **Step 2: Verify it fails**

Run: `cargo test -p hector-core --test ast_engine ast_violation_populates_column_and_context`
Expected: FAIL — both fields are `None`.

- [ ] **Step 3: Update `engine/ast.rs:52-69`** to return both fields. Pull the node out of the iterator and extract column + context:

```rust
fn find_first_match(content: &str, pattern_str: &str, lang_name: &str)
    -> Result<Option<(u32, u32, String)>>
{
    use ast_grep_core::matcher::Pattern;
    use ast_grep_language::{LanguageExt, SupportLang};
    use std::str::FromStr;
    let lang = SupportLang::from_str(lang_name)
        .map_err(|_| anyhow!("unknown ast-grep language: {lang_name}"))?;
    let grep = lang.ast_grep(content);
    let pattern = Pattern::try_new(pattern_str, lang)
        .map_err(|e| anyhow!("invalid ast-grep pattern `{pattern_str}`: {e:?}"))?;
    let Some(node) = grep.root().find_all(pattern).next() else { return Ok(None) };
    let line = (node.start_pos().line() + 1) as u32;
    let column = (node.start_pos().column() + 1) as u32;
    let lines: Vec<&str> = content.lines().collect();
    let idx = (line - 1) as usize;
    let lo = idx.saturating_sub(3);
    let hi = (idx + 4).min(lines.len());
    let ctx = lines[lo..hi].join("\n");
    Ok(Some((line, column, ctx)))
}
```

Update `run` to use the new triple and set the violation fields:

```rust
let Some((line, column, ctxstr)) = find_first_match(content, pattern_str, lang_name)? else {
    return Ok(None);
};
// ... existing severity match
Ok(Some(Violation {
    rule_id: ctx.rule_id.to_string(),
    severity,
    engine: Engine::Ast,
    file: ctx.file.display().to_string(),
    line: Some(line),
    column: Some(column),
    message: ctx.rule.description.clone(),
    suggestion: ctx.rule.fix_hint.clone(),
    context: Some(ctxstr),
}))
```

- [ ] **Step 4: Verify the test passes**

Run: `cargo test -p hector-core --test ast_engine`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-core/src/engine/ast.rs crates/hector-core/tests/ast_engine.rs
git commit -m "$(cat <<'EOF'
verdict(shape): populate column and context for AST violations (P1-3)

Both fields were defined in the locked verdict shape but always None.
AST has column data from start_pos() and we can synthesize ±3 lines of
context around the match. Script, semantic, and session engines leave
column None where they have no positional info — explicit None is fine,
silent None pretending data was lost is not.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6 — Disable + LLM Reliability (P1-2, P1-5, P1-6, P1-12)

Four disjoint files; four parallel subagents possible.

### Task 6.1: File-level `hector-disable:` honors `v.line == None` (P1-2) [parallel]

**Files:** `crates/hector-core/src/disable.rs:21-26`, `crates/hector-core/src/runner.rs:209-214`, `crates/hector-core/tests/disable.rs`

- [ ] **Step 1: Write failing test** in `tests/disable.rs`:

```rust
#[test]
fn file_level_disable_silences_script_violation_without_line() {
    use hector_core::disable::DisableMap;
    let src = "// hector-disable: noisy-script\nfn main() {}\n";
    let map = DisableMap::from_source(src);
    // New API: ask whether a rule is disabled file-wide (line: None).
    assert!(map.is_disabled_file_wide("noisy-script"));
    assert!(!map.is_disabled_file_wide("other-rule"));
}
```

- [ ] **Step 2: Implement** in `disable.rs`:

```rust
impl DisableMap {
    pub fn is_disabled_file_wide(&self, rule_id: &str) -> bool {
        self.by_line.values().any(|rs| rs.iter().any(|r| r == rule_id))
    }
}
```

In `runner.rs:209-214`, change:

```rust
Ok(Some(v)) => {
    let disabled = match v.line {
        Some(line) => disable_map.is_disabled(line, rule_id),
        None => disable_map.is_disabled_file_wide(rule_id),
    };
    if disabled {
        passed.push(rule_id.clone());
        continue;
    }
    violations.push(v);
}
```

- [ ] **Step 3: Verify & commit** (`cargo test -p hector-core --test disable`, then commit with the standard "fix(disable): apply directives to file-level violations (P1-2)" message).

### Task 6.2: LLM unknown status → engine error, not silent violation (P1-5) [parallel]

**Files:** `crates/hector-core/src/llm/mod.rs:113-141`, `crates/hector-core/tests/llm_factory.rs`

- [ ] **Step 1: Write failing test** in `tests/llm_factory.rs`:

```rust
#[test]
fn parse_verdicts_returns_err_on_unknown_status() {
    let body = r#"[{"rule_id": "r1", "status": "Pass"}]"#;
    let err = hector_core::llm::parse_verdicts(body)
        .expect_err("unknown casing must be an error, not a violation");
    let s = format!("{err:#}");
    assert!(s.contains("Pass") || s.contains("unknown status"));
}

#[test]
fn parse_verdicts_lowercases_status() {
    // "pass" and "PASS" should both succeed as Pass after lowercasing.
    let body = r#"[{"rule_id":"r1","status":"pass"},{"rule_id":"r2","status":"PASS"}]"#;
    let v = hector_core::llm::parse_verdicts(body).unwrap();
    assert!(matches!(v[0].status, hector_core::llm::RuleStatus::Pass));
    assert!(matches!(v[1].status, hector_core::llm::RuleStatus::Pass));
}
```

- [ ] **Step 2: Implement** at `llm/mod.rs:128`:

```rust
status: match w.status.to_ascii_lowercase().as_str() {
    "pass" => RuleStatus::Pass,
    "violation" => RuleStatus::Violation {
        message: w.message.unwrap_or_default(),
        line: w.line,
    },
    other => bail!("unknown LLM status `{other}` for rule {}", w.rule_id),
},
```

Make `parse_verdicts` return `Result<...>` for each parse; today the closure inside `.map` can't bail. Refactor to a manual loop:

```rust
let mut out = Vec::with_capacity(wire.len());
for w in wire {
    let status = match w.status.to_ascii_lowercase().as_str() {
        "pass" => RuleStatus::Pass,
        "violation" => RuleStatus::Violation { message: w.message.unwrap_or_default(), line: w.line },
        other => bail!("unknown LLM status `{other}` for rule {}", w.rule_id),
    };
    out.push(RuleVerdict { rule_id: w.rule_id, status });
}
Ok(out)
```

- [ ] **Step 3: Verify & commit** (standard flow).

### Task 6.3: LLM rule_id mismatch → engine error (P1-6) [parallel]

**Files:** `crates/hector-core/src/engine/semantic.rs:19-21`, `crates/hector-core/src/engine/session.rs:24-26`, `crates/hector-core/tests/semantic_engine.rs`, `crates/hector-core/tests/session_engine.rs`

- [ ] **Step 1: Write failing test** in `tests/semantic_engine.rs` using `wiremock` to stub an LLM that returns a hallucinated `rule_id`:

```rust
use wiremock::matchers::*;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn semantic_engine_errors_on_rule_id_mismatch() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [{"type":"text","text":"[{\"rule_id\":\"hallucinated\",\"status\":\"pass\"}]"}]
        })))
        .mount(&server)
        .await;
    let client = hector_core::llm::AnthropicClient::new("k", "m", Some(server.uri()));
    let rule = sample_rule();
    let ctx = hector_core::engine::RuleContext {
        rule_id: "expected-id",
        rule: &rule,
        file: std::path::Path::new("/tmp/x.rs"),
        content: Some("fn main() {}\n"),
        diff: Some("--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-x\n+y\n"),
        cwd: std::path::Path::new("/tmp"),
        llm: Some(&client),
    };
    use hector_core::engine::RuleEngine;
    let err = hector_core::engine::semantic::SemanticEngine.run(&ctx).expect_err("must error on mismatch");
    assert!(format!("{err:#}").contains("expected-id"));
}
```

(`sample_rule()` — copy the one from `llm/prompt.rs:218`. `Cargo.toml` already has `wiremock` as a dev-dep.)

- [ ] **Step 2: Implement** — in `engine/semantic.rs:19-21` and `engine/session.rs:24-26`, replace the `else { return Ok(None) }` with:

```rust
let Some(v) = verdicts.into_iter().find(|v| v.rule_id == ctx.rule_id) else {
    return Err(anyhow!(
        "LLM returned no verdict for rule `{}`; got {} other verdicts",
        ctx.rule_id, /* count from a clone before consumption */
    ));
};
```

Refactor to capture the count before the iterator is consumed.

- [ ] **Step 3: Verify & commit.**

### Task 6.4: Script timeout + output cap (P1-12) [parallel — independent module]

**Files:** `crates/hector-core/src/engine/capability.rs:51`, `crates/hector-core/Cargo.toml` (add `wait-timeout = "0.2"`)

- [ ] **Step 1: Write failing test** in `tests/script_engine.rs`:

```rust
#[test]
fn script_engine_kills_runaway_command() {
    use hector_core::config::{Capabilities, WritesPolicy};
    use hector_core::engine::capability::run_with_capabilities;
    let start = std::time::Instant::now();
    let caps = Capabilities { network: false, writes: WritesPolicy::Unrestricted };
    // 30s sleep but the runner should kill it well before then.
    let out = run_with_capabilities("sleep 30", std::path::Path::new("/tmp"), &caps).unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(10), "runaway must be killed");
    assert!(out.exit_code != 0, "killed process must report non-zero");
}
```

- [ ] **Step 2: Implement** — switch from `child.output()` to a piped spawn with `wait_timeout` and bounded reads:

```rust
use std::io::Read;
use std::time::Duration;
use wait_timeout::ChildExt;

const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_OUTPUT: usize = 1 << 20; // 1 MiB per stream

let mut child = Command::new("sh")
    .arg("-c").arg(cmd).current_dir(cwd)
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .spawn().context("spawn")?;

let status = match child.wait_timeout(TIMEOUT).context("wait_timeout")? {
    Some(s) => s,
    None => { child.kill().ok(); child.wait().ok(); return Ok(ExecOutcome {
        stdout: String::new(),
        stderr: format!("hector: script killed after {TIMEOUT:?}"),
        exit_code: 124,
    });}
};
let mut stdout = String::new();
child.stdout.take().unwrap().take(MAX_OUTPUT as u64).read_to_string(&mut stdout).ok();
let mut stderr = String::new();
child.stderr.take().unwrap().take(MAX_OUTPUT as u64).read_to_string(&mut stderr).ok();
Ok(ExecOutcome { stdout, stderr, exit_code: status.code().unwrap_or(-1) })
```

- [ ] **Step 3: Verify & commit.**

---

## Phase 7 — LLM, Adapters, Telemetry, AST (P1-7, P1-8, P1-9, P1-10, P1-11)

Four parallelizable tasks across disjoint files.

### Task 7.1: LLM client timeouts + bounded retry (P1-7) [parallel]

**Files:** `crates/hector-core/src/llm/anthropic.rs:25`, `crates/hector-core/src/llm/openai_compat.rs:35`, `crates/hector-core/tests/anthropic.rs`, `crates/hector-core/tests/openai_compat.rs`

- [ ] **Step 1: Write failing test** in `tests/anthropic.rs` using wiremock with a `delay` that exceeds the configured timeout:

```rust
#[tokio::test]
async fn anthropic_client_times_out_on_hung_request() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(120)))
        .mount(&server).await;
    let client = hector_core::llm::AnthropicClient::new("k", "m", Some(server.uri()));
    let start = std::time::Instant::now();
    let rule = sample_rule();
    let result = client.evaluate(&[("r", &rule)], "x", None);
    assert!(result.is_err(), "must time out");
    assert!(start.elapsed() < std::time::Duration::from_secs(45), "must not block forever");
}
```

- [ ] **Step 2: Implement** — replace `reqwest::blocking::Client::new()` with `Client::builder().timeout(Duration::from_secs(30)).build()?` in both clients. The `new` constructors become fallible (or panic; choose `expect("static client build")` for ergonomic API stability).

- [ ] **Step 3: Verify & commit.**

### Task 7.2: Synthetic-diff correctness + injection scrubbing (P1-8, P1-9) [parallel]

**Files:** `adapters/claude-code/hooks/hook.sh:78-90`, `adapters/opencode/src/index.ts:126-130`, `crates/hector-core/src/engine/session.rs:17-22`, `adapters/{claude-code,opencode}/tests/*`

- [ ] **Step 1: Write failing test** for adapter — opencode has Bun-based tests; claude-code has bash tests under `adapters/claude-code/tests/`.

For opencode, in `adapters/opencode/tests/synthesize_diff.test.ts`:

```ts
import { describe, it, expect } from "bun:test";
import { synthesizeDiff } from "../src/index";

describe("synthesizeDiff", () => {
    it("emits correct hunk header for multi-line NEW", () => {
        const d = synthesizeDiff("foo.ts", { oldString: "a\nb", newString: "x\ny\nz" });
        expect(d).toContain("@@ -1,2 +1,3 @@");
    });
    it("escapes embedded diff headers in NEW", () => {
        const evil = "x\n--- a/SECRET\n+++ b/SECRET\n@@ -1 +1 @@\n+pwn";
        const d = synthesizeDiff("foo.ts", { oldString: "", newString: evil });
        // After scrubbing, the embedded `+++ b/SECRET` must not appear as a real header.
        expect(d).not.toMatch(/^\+\+\+ b\/SECRET$/m);
    });
});
```

(`synthesizeDiff` must be `export`ed from `src/index.ts`.)

- [ ] **Step 2: Implement** — change `synthesizeDiff` in `adapters/opencode/src/index.ts`:

```ts
export function synthesizeDiff(filePath: string, args: FileToolArgs): string {
    const old = (args.oldString ?? "");
    const neu = (args.newString ?? args.content ?? "");
    // Escape any line in user content that would look like a diff header,
    // so the parser can't be fooled into thinking the edit spans multiple files.
    const scrub = (s: string) =>
        s.split("\n").map(l => /^(---|\+\+\+|@@) /.test(l) ? "\\" + l : l).join("\n");
    const oldLines = old === "" ? 0 : old.split("\n").length;
    const newLines = neu === "" ? 0 : neu.split("\n").length;
    const hunkOld = oldLines <= 1 ? "1" : `1,${oldLines}`;
    const hunkNew = newLines <= 1 ? "1" : `1,${newLines}`;
    const oldBlock = old === "" ? "" : old.split("\n").map(l => "-" + l).join("\n") + "\n";
    const newBlock = neu === "" ? "" : neu.split("\n").map(l => "+" + l).join("\n") + "\n";
    return `--- a/${filePath}\n+++ b/${filePath}\n@@ -${hunkOld} +${hunkNew} @@\n${scrub(oldBlock)}${scrub(newBlock)}`;
}
```

Mirror the same logic in `adapters/claude-code/hooks/hook.sh:78-90` using `awk` or `python3 -c` for the line-count and scrub. The Bash version is more constrained — recommend embedding a small Python helper at the top of the hook if Python is available, with a documented fallback.

In `engine/session.rs:17-22`, swap the colon-delimited filename header for one less likely to appear in user content. Easiest: include the session id at construction time:

```rust
let aggregated = state
    .edits
    .iter()
    .map(|e| format!("--- file:{}:{} ---\n{}", state.session_id, e.file, e.diff))
    .collect::<Vec<_>>()
    .join("\n\n");
```

Using a per-session id derived at random (already done — `SessionState::new` adds a timestamp) makes the delimiter unpredictable. The narrower fix — escape literal `--- file:` substrings in `e.diff` and `e.file` before joining — is also acceptable.

- [ ] **Step 3: Verify & commit.**

### Task 7.3: Telemetry atomicity + permissions (P1-10, P2-16) [parallel]

**Files:** `crates/hector-core/src/telemetry.rs:17-25`, `crates/hector-core/tests/telemetry.rs`

- [ ] **Step 1: Write failing test** — concurrent writers from threads, then assert every line is valid JSON:

```rust
#[test]
fn telemetry_append_is_atomic_under_concurrent_writers() {
    use std::thread;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("log.jsonl");
    let handles: Vec<_> = (0..16).map(|i| {
        let p = path.clone();
        thread::spawn(move || {
            for j in 0..100 {
                let entry = hector_core::telemetry::LogEntry {
                    timestamp: "t".into(),
                    kind: "check".into(),
                    file: format!("file-{i}-{j}-{}", "x".repeat(8192)),
                    rule_id: None,
                    status: "pass".into(),
                    elapsed_ms: 0,
                };
                hector_core::telemetry::append(&p, &entry).unwrap();
            }
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
    let content = std::fs::read_to_string(&path).unwrap();
    for (i, line) in content.lines().enumerate() {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|e| panic!("line {i} not valid JSON: {e}\n{line}"));
    }
}
```

- [ ] **Step 2: Implement** — write `entry + "\n"` as one `write_all` and set mode 0600 on create:

```rust
pub fn append(path: &Path, entry: &LogEntry) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut opts = OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');
    // For entries > 4KiB (PIPE_BUF), atomic append is not guaranteed.
    // Use flock to serialize writers crossing that threshold.
    #[cfg(unix)]
    {
        if line.len() > 4096 {
            use std::os::fd::AsRawFd;
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            let r = file.write_all(line.as_bytes());
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
            r?;
        } else {
            file.write_all(line.as_bytes())?;
        }
    }
    #[cfg(not(unix))]
    file.write_all(line.as_bytes())?;
    Ok(())
}
```

(libc was added to top-level deps in Phase 3 Task 3.1.)

- [ ] **Step 3: Verify & commit.**

### Task 7.4: AST collects every match (P1-11) [parallel]

**Files:** `crates/hector-core/src/engine/ast.rs:13-69`, `crates/hector-core/src/engine/mod.rs` (signature), every `RuleEngine` impl, `crates/hector-core/src/runner.rs` (use the vec), `tests/ast_engine.rs`

This is the largest signature change: `RuleEngine::run` returns `Vec<Violation>` not `Option<Violation>`. Touches every engine and the runner loop.

- [ ] **Step 1: Write failing test** in `tests/ast_engine.rs`:

```rust
#[test]
fn ast_returns_every_match_not_just_first() {
    let rule = hector_core::config::Rule {
        description: "no unwrap".into(),
        engine: hector_core::config::EngineKind::Ast,
        scope: vec!["**/*.rs".into()],
        severity: hector_core::config::Severity::Warning,
        script: None,
        pattern: Some("$E.unwrap()".into()),
        language: Some("rust".into()),
        context: None,
        capabilities: None,
        fix_hint: None,
    };
    let content = "fn a() { x.unwrap(); y.unwrap(); z.unwrap(); }\n";
    let ctx = hector_core::engine::RuleContext {
        rule_id: "no-unwrap",
        rule: &rule,
        file: std::path::Path::new("test.rs"),
        content: Some(content),
        diff: None,
        cwd: std::path::Path::new("."),
        llm: None,
    };
    use hector_core::engine::RuleEngine;
    let vs = hector_core::engine::ast::AstEngine.run(&ctx).unwrap();
    assert_eq!(vs.len(), 3, "all three .unwrap()s must be reported, got {vs:?}");
}
```

- [ ] **Step 2: Implement** — change `engine/mod.rs`:

```rust
pub trait RuleEngine {
    fn run(&self, ctx: &RuleContext) -> Result<Vec<Violation>>;
}
```

Update every impl (`AstEngine`, `ScriptEngine`, `SemanticEngine`) to return `Vec` (single-element for script/semantic, multi-element for AST). `SessionEngine::evaluate` keeps its `Option` shape because it's a per-rule API.

Update `runner.rs:163-232` to handle the vec:

```rust
match outcome {
    Ok(vs) if vs.is_empty() => passed.push(rule_id.clone()),
    Ok(vs) => {
        for v in vs {
            let disabled = match v.line {
                Some(line) => disable_map.is_disabled(line, rule_id),
                None => disable_map.is_disabled_file_wide(rule_id),
            };
            if disabled { continue; }
            violations.push(v);
        }
    }
    Err(e) => { /* unchanged */ }
}
```

- [ ] **Step 3: Verify & commit.** Run the full suite — this touches many call sites.

---

## Phase 8 — P2 Cleanup (22 items)

Each item is small enough to fit in 1–3 commits with a single test. Cluster by file for parallel dispatch. Each item below names: ID, file(s), test approach, fix one-liner.

### Cluster 8A — Session state hardening (P2-1, P2-2, P2-12, P2-18)

- [ ] **P2-1 session.json read-modify-write race** in `crates/hector-cli/src/commands/session.rs:5-21`. Test: spawn 32 threads each calling `record` with a distinct `EditRecord`; assert final `session.json` contains all 32. Fix: `fs2::FileExt::lock_exclusive` on a sibling `.lock` file around load → mutate → save.
- [ ] **P2-2 `SessionState::load` errors on missing file** at `session_state.rs:29-35`. Test: call `load` on a path that doesn't exist; expect empty `SessionState`. Fix:
  ```rust
  pub fn load(path: &Path) -> Result<Self> {
      if !path.exists() { return Ok(Self::new("")); }
      // ... existing
  }
  ```
- [ ] **P2-12 session cleared on Block** at `commands/check.rs:31`. Test: assert that after a Block verdict, the session file still exists. Fix: gate `clear` on `Status::Pass | Status::Warn`.
- [ ] **P2-18 unbounded session.json growth** at `session_state.rs:55-57`. Test: append 10k records; assert the file is rotated (or capped). Fix: cap `state.edits.len()` (e.g., keep the most recent 1000) before save, with a stderr warning on drop.

### Cluster 8B — Trust & migrate UX (P2-3, P2-7, P2-8, P2-11)

- [ ] **P2-3 TOCTOU between trust verify and config parse** in `runner.rs:58-71` + `extends.rs:20`. Test: race-prone integration test is flaky; instead, refactor and assert `parse_file_with_extends(content: &str, …)` accepts pre-loaded content. Fix: read once, pass bytes to both verify and parser.
- [ ] **P2-7 `hector trust` destroys comments** in `trust.rs:71-81`. Test: assert that `write_trust_block("# leading comment\nschema_version: 2\n…")` preserves the comment. Fix: locate existing `trust:` block via line regex; edit in place; preserve everything else verbatim. If no `trust:` block, append it at EOF.
- [ ] **P2-8 `hector migrate` naive string replace** in `commands/migrate.rs:20`. Test: assert that a `.bully.yml` with `# schema_version: 1` in a comment doesn't get touched. Fix: parse YAML, set `schema_version: 2`, re-serialize.
- [ ] **P2-11 v1 schema accepted but unloadable** in `runner.rs:60-73`. Test: load a v1 config; expect a clean "run hector migrate" error before trust verify. Fix: detect `schema_version == 1` at parse time *before* `trust::verify`.

### Cluster 8C — Baseline + diff polish (P2-4, P2-5, P2-6, P2-10, P1-4)

- [ ] **P1-4 baseline fingerprint collision** in `baseline.rs:29-30`. Test: assert `fingerprint(rule_id="a::b", file="c")` ≠ `fingerprint(rule_id="a", file="b::c")`. Fix: JSON-encode the tuple: `serde_json::to_string(&(rule_id, file, line))`.
- [ ] **P2-4 disable parser breaks on `/`** in `disable.rs:56-58,63`. Test: `hector-disable: python/no-print` must yield the full id. Fix: only treat `/` as terminator when followed by `/` or `*`.
- [ ] **P2-5 baseline non-atomic write** in `baseline.rs:21-26`. Test: simulate a crash mid-write (write to a sibling temp, kill, assert baseline still loads). Fix: temp-file-rename plus `File::sync_all`.
- [ ] **P2-6 baseline corruption silently empty** in `runner.rs:234-236`. Test: write garbage to `baseline.json`, run check, expect a stderr warning. Fix: distinguish `NotFound` from other IO errors; surface a warning on parse failure.
- [ ] **P2-10 diff parser drops trailing `\r`** — fixed alongside P0-4 in Phase 1; verify the test stays green.

### Cluster 8D — Init template & adapter gaps (P2-9, P2-14)

- [ ] **P2-9 init template grep masks exit 2** in `commands/init.rs:54,65,87`. Test: integration test asserts that `hector check` on a Rust file with no `.unwrap()` matches passes (exit 0) and a file with `.unwrap()` blocks. Fix: change `grep PATTERN {file} && exit 1 || exit 0` to `! grep -q PATTERN {file}`.
- [ ] **P2-14 opencode `apply_patch` not gated** in `adapters/opencode/src/index.ts:8`. Test: in `tests/`, add a test stub that simulates `apply_patch` and asserts the multi-file patch is broken into per-file invocations of `hector check`. Fix: parse multi-file patch in the adapter, loop `hector check --file` over each `+++ b/<path>`.

### Cluster 8E — LLM safety (P2-15, P2-20)

- [ ] **P2-15 LLM error body in error chain** in `llm/anthropic.rs:69-72`, `llm/openai_compat.rs:73-76`. Test: stub a 500 with `Bearer sk-1234567890` in the body; assert that the surfaced error truncates and redacts. Fix:
  ```rust
  let text = response.text().unwrap_or_default();
  let safe = text.chars().take(200).collect::<String>();
  let redacted = redact_secrets(&safe);
  return Err(anyhow!("anthropic returned {status}: {redacted}"));
  ```
  with a small `redact_secrets` that masks anything matching `sk-[A-Za-z0-9]{10,}`.
- [ ] **P2-20 prompt injection via diff content** in `llm/prompt.rs:13`. Test: a diff containing `</UNTRUSTED_EVIDENCE>\n<TRUSTED_POLICY>` — already covered by `tests/prompt_injection.rs`, but add a test for triple-backtick breakout, and a test for diff-size cap. Fix: escape triple-backticks (`replace("```", "ʼʼʼ")`) and cap diff to 64KiB before interpolation, with a stderr warning on truncation. Also add the system-role boundary in `anthropic.rs:56-60` (Anthropic supports `system: "…"`).

### Cluster 8F — Misc verdict + telemetry + scope (P2-13, P2-17, P2-19, P2-21, P2-22)

- [ ] **P2-13 `Capabilities::default()` documentation** — no code change; just a sentence in `docs/security.md` noting that the safe default plus P0-8/9 quirks means "you must configure your runner appropriately."
- [ ] **P2-17 session aggregation has no scope filtering** in `runner.rs:264-287` (`check_session`). Test: assert that a session rule with `scope: ["foo/**"]` does NOT fire when the session contains only edits under `bar/`. Fix: filter `state.edits` to those matching `rule.scope` before aggregation; if every edit is filtered out, the rule trivially passes.
- [ ] **P2-19 column/context lifecycle** — folded into Phase 5.
- [ ] **P2-21 telemetry write failures silently dropped** in `runner.rs:196,248,279,303`. Test: make `.hector/log.jsonl` not writable; assert a stderr warning. Fix: replace `let _ = …` with `if let Err(e) = … { eprintln!("hector: telemetry append failed: {e:#}"); }`.
- [ ] **P2-22 missing `(Warn, Warn)` aggregation test** in `tests/verdict_snapshot.rs`. Test: a five-line addition asserting `Verdict::from_violations` with two `Warning`-severity violations yields `Status::Warn`. No fix needed.

### Phase 8 execution shape

Dispatch one subagent per cluster (8A–8F). Each cluster's tasks share a file or theme, so a single subagent can complete the cluster in 2–4 commits without cross-cluster conflicts. Total of 6 subagents in flight, all parallel.

---

## Final Verification

After all phases are merged:

- [ ] `cargo test --workspace`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo fmt --check`
- [ ] `cargo insta test` then `cargo insta review` (snapshots should be intentional after Phase 5)
- [ ] Manual end-to-end: in a temp project with absolute paths, run the Claude Code adapter against a synthetic edit; assert a rule fires that previously didn't.
- [ ] Run the dogfood `scripts/dogfood.sh` if it exists; assert exit 0 on a clean tree and exit 2 on a planted violation.
- [ ] Re-read `docs/2026-05-12-bug-audit.md` and confirm every finding is closed. The "Rejected first-pass claims" section needs no action.

---

## Subagent Dispatch Recipe (executor reference)

Use the `superpowers:subagent-driven-development` skill as the parent loop. Send one subagent per task with this prompt template:

```
You are implementing Task <N.M> from docs/superpowers/plans/2026-05-12-bug-audit-remediation.md.

Goal: <single sentence from task header>.

Files in scope: <list from task>.

You MUST follow Test-Driven Development (project CLAUDE.md rule): write the failing test first, prove it fails, then implement, prove it passes. Use the `superpowers:test-driven-development` skill.

Touch only the files listed for your task. If you find work outside scope, leave a note in your final summary; do not edit.

When done, run:
  cargo test --workspace
  cargo clippy --all-targets -- -D warnings
  cargo fmt --check

Then commit using the message template in the plan. Report exit codes and test counts in your final summary.
```

Parallelism guardrails:
- Phase 0 alone: one subagent.
- Phase 1: three subagents (Tasks 1.1, 1.2, 1.3) — disjoint files except for `runner.rs` which Task 1.1 also touches. Sequence: 1.1 first, then 1.2 and 1.3 in parallel. *(Even simpler: serialize all three within Phase 1 if merge conflicts are costly.)*
- Phase 2: one subagent (Tasks 2.1 is a single coordinated change).
- Phase 3: one subagent for 3.1, then 3.2.
- Phase 4: one subagent.
- Phase 5: two subagents (5.1 and 5.2), parallel.
- Phase 6: four subagents, parallel.
- Phase 7: four subagents, parallel.
- Phase 8: six subagents (one per cluster), parallel.

Two-stage review: after each subagent reports done, the parent runs `cargo test --workspace` itself, reviews the diff, and either accepts or sends a follow-up. Don't merge phases out of order even if subagents finish fast.

---

## Estimated effort

- P0 (Phases 0–4): 10 tasks; ~1–2 dev-days each fully tested; serial-first then parallel = ~5 working days for one engineer, ~2 working days with three subagents in flight.
- P1 (Phases 5–7): 11 tasks; mostly parallel; ~3 working days with full fan-out.
- P2 (Phase 8): 22 tasks bundled into 6 clusters; ~2 working days with six subagents.

Total: ~7 working days with aggressive parallelism, ~10 days solo.
