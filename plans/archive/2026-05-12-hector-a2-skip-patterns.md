# Hector A2 — Built-in Skip Patterns + Project `skip:` + `~/.hector-ignore`

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or superpowers:subagent-driven-development) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec section:** [`specs/2026-05-12-bully-parity-closures.md` §A2](../specs/2026-05-12-bully-parity-closures.md)
**Severity:** 🔴 critical (cost lever)
**Sequencing:** Second item in the 0.2.0 cohort (after [A1](2026-05-12-hector-a1-prompt-injection.md)).

---

**Goal:** Stop running rules — especially expensive `engine: semantic` LLM rules — against lockfiles, generated code, and build artifacts. Default the matcher to bully's well-known list (lockfiles, minified, dist/build/__pycache__/node_modules, generated markers); let users add to it via top-level `skip:` in `.hector.yml` or via `~/.hector-ignore`.

**Architecture:** A new `crates/hector-core/src/config/skip.rs` module exports `built_in_skip_globs() -> &'static [&'static str]` and a `SkipMatcher` wrapping a `globset::GlobSet`. The matcher follows the same right-anchored convention as `ScopeMatcher` (bare `*.lock` matches at any depth). `Config` gains an additive `skip: Vec<String>` field; `extends:` resolution merges inherited `skip:` lists. `HectorEngine::load_with` builds the effective skip list (built-ins ∪ ~/.hector-ignore ∪ project ∪ extends) once and stashes a `SkipMatcher` on the engine. `HectorEngine::check` consults the matcher first thing — on match, the function returns `Verdict::pass()` with empty violations/passed, writes a `kind: "skipped"` telemetry record, and never enters the rule loop. Per the spec's open question, no new `Status::Skipped` enum variant is added in 0.2 — fold into `Pass`. Reconsider before the 0.3 verdict freeze.

**Tech Stack:** Rust, `globset` (already a workspace dep, used by `ScopeMatcher`), no new deps. Tests use `tempfile` (already a dev-dep) for `~/.hector-ignore` simulation via `HOME` env override.

---

## Decisions ratified up-front (per spec §3 + §A2 + open questions)

| Decision | Choice | Reason |
|---|---|---|
| Built-ins replaceable vs additive | **Additive** (built-ins always apply; `skip:` adds) | Spec default; matches bully. A `skip.replace_builtins: true` flag is rejected unless someone has a real need. |
| `Status::Skipped` in verdict enum | **Fold into `Pass`** for 0.2 | Avoids a `SCHEMA_VERSION` bump (verdict locks at 0.3). Distinguish via telemetry `kind: "skipped"` instead. |
| Extends + `skip:` merge | **Inherited skip lists are unioned** with local | Mirrors bully's `collect_skip_with_extends`. Skip entries are commutative — no "local wins on collision" because the values are not keyed. |
| `~/.hector-ignore` toggle | Always honored if present; absent is silent no-op | Spec default. No CLI flag to disable for 0.2 (yagni). |
| Telemetry record shape | Use existing flat `LogEntry { kind: "skipped", status: "pass", rule_id: None, … }` | D1 (typed telemetry) hasn't shipped; using the flat shape keeps the change isolated. When D1 lands, the typed `LogEntry::FileSkipped { … reason }` variant supersedes this. Document the upgrade path in the D1 plan. |
| Skip-reason storage | **Not in verdict at 0.2.** Per-rule `semantic_skipped` reasons (A3) and per-file skip reasons go through telemetry only | Verdict shape is precious; telemetry is cheap. Users inspect via `cat .hector/log.jsonl | jq`. |
| Trust-fingerprint impact | **None for users who don't add `skip:`**; **fingerprint changes** (correctly) for users who do | The fingerprint is computed over the raw YAML, not the parsed Config struct (see `trust::canonicalize_for_fingerprint`). Adding the `skip:` field to Rust is invisible to fingerprints; adding `skip:` to a YAML config is a deliberate change that warrants `hector trust`. |

If anything in this table feels wrong on a fresh read, raise it before Task 4 — it sets the schema and runner short-circuit shape.

---

## File structure

```
crates/hector-core/
├── src/
│   ├── config/
│   │   ├── mod.rs                  ← MODIFIED: pub mod skip; re-export SkipMatcher
│   │   ├── skip.rs                 ← NEW: built_in_skip_globs() + SkipMatcher + parse_user_global_ignore()
│   │   ├── types.rs                ← MODIFIED: add `skip: Vec<String>` to Config
│   │   └── extends.rs              ← MODIFIED: union skip lists across extends
│   └── runner.rs                   ← MODIFIED: build SkipMatcher in load_with; short-circuit in check()
└── tests/
    ├── skip_matcher.rs             ← NEW: unit + integration tests for built-ins + project + ignore-file merging
    └── runner_skip.rs              ← NEW: end-to-end test that Cargo.lock is skipped, verdict shape is right, telemetry recorded
```

CLI test for the self-check (running `hector check --file Cargo.lock` against hector itself) goes in `crates/hector-cli/tests/cli_check.rs`.

---

## Phase 1 — Built-in patterns + `SkipMatcher` (TDD)

### Task 1: Failing unit tests for the skip module

**Files:**
- Create: `crates/hector-core/src/config/skip.rs`

The module doesn't exist yet; tests will fail to compile. Add the file with **only** `#[cfg(test)] mod tests` content for now — no implementation. We'll fill it in Task 2.

- [x] **Step 1: Create the test scaffold**

```rust
//! File-skip patterns: built-in defaults + project `skip:` list + user-global ignore file.

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn built_in_skip_globs_contains_lockfiles() {
        let globs = built_in_skip_globs();
        assert!(globs.contains(&"Cargo.lock"));
        assert!(globs.contains(&"package-lock.json"));
        assert!(globs.contains(&"yarn.lock"));
        assert!(globs.contains(&"pnpm-lock.yaml"));
        assert!(globs.contains(&"poetry.lock"));
        assert!(globs.contains(&"bun.lock"));
        assert!(globs.contains(&"Pipfile.lock"));
    }

    #[test]
    fn built_in_skip_globs_contains_minified_assets() {
        let globs = built_in_skip_globs();
        assert!(globs.contains(&"*.min.js"));
        assert!(globs.contains(&"*.min.css"));
    }

    #[test]
    fn built_in_skip_globs_contains_build_dirs() {
        let globs = built_in_skip_globs();
        assert!(globs.contains(&"dist/**"));
        assert!(globs.contains(&"build/**"));
        assert!(globs.contains(&"__pycache__/**"));
        assert!(globs.contains(&"node_modules/**"));
        assert!(globs.contains(&"target/**"));
        assert!(globs.contains(&".next/**"));
        assert!(globs.contains(&".nuxt/**"));
    }

    #[test]
    fn built_in_skip_globs_contains_generated_markers() {
        let globs = built_in_skip_globs();
        assert!(globs.contains(&"*.generated.*"));
        assert!(globs.contains(&"*.pb.go"));
        assert!(globs.contains(&"*.g.dart"));
        assert!(globs.contains(&"*.freezed.dart"));
    }

    #[test]
    fn matcher_skips_cargo_lock_at_root() {
        let m = SkipMatcher::with_built_ins(&[]).unwrap();
        assert!(m.matches(Path::new("Cargo.lock")));
    }

    #[test]
    fn matcher_skips_cargo_lock_in_subdir() {
        let m = SkipMatcher::with_built_ins(&[]).unwrap();
        assert!(m.matches(Path::new("crates/hector-core/Cargo.lock")));
    }

    #[test]
    fn matcher_skips_node_modules_dir_recursively() {
        let m = SkipMatcher::with_built_ins(&[]).unwrap();
        assert!(m.matches(Path::new("node_modules/foo/index.js")));
        assert!(m.matches(Path::new("packages/web/node_modules/bar.js")));
    }

    #[test]
    fn matcher_does_not_skip_normal_source() {
        let m = SkipMatcher::with_built_ins(&[]).unwrap();
        assert!(!m.matches(Path::new("src/main.rs")));
        assert!(!m.matches(Path::new("crates/hector-core/src/runner.rs")));
        assert!(!m.matches(Path::new("README.md")));
    }

    #[test]
    fn matcher_honors_extra_user_globs() {
        let m = SkipMatcher::with_built_ins(&["*.snap".into(), "fixtures/**".into()]).unwrap();
        assert!(m.matches(Path::new("tests/foo.snap")));
        assert!(m.matches(Path::new("crates/x/tests/bar.snap")));
        assert!(m.matches(Path::new("fixtures/large.json")));
    }

    #[test]
    fn parse_user_global_ignore_strips_blanks_and_comments() {
        let raw = "\
# my ignore file
*.snap

# blank lines above and below allowed

  *.bak  
fixtures/**
";
        let globs = parse_user_global_ignore(raw);
        assert_eq!(
            globs,
            vec!["*.snap".to_string(), "*.bak".to_string(), "fixtures/**".to_string()]
        );
    }

    #[test]
    fn parse_user_global_ignore_empty_input_is_empty_vec() {
        assert!(parse_user_global_ignore("").is_empty());
        assert!(parse_user_global_ignore("\n\n#only comments\n\n").is_empty());
    }
}
```

- [x] **Step 2: Wire the module into `config/mod.rs`**

```rust
// In crates/hector-core/src/config/mod.rs, add to the existing module list:
pub mod skip;
```

- [x] **Step 3: Run the failing tests**

Run: `cargo test -p hector-core --lib config::skip`
Expected: compile error — `built_in_skip_globs`, `SkipMatcher`, `parse_user_global_ignore` not found. Good.

---

### Task 2: Implement the skip module

**Files:**
- Modify: `crates/hector-core/src/config/skip.rs` (prepend implementation above the test module)

- [x] **Step 1: Add the implementation**

```rust
//! File-skip patterns: built-in defaults + project `skip:` list + user-global ignore file.
//!
//! Mirrors bully's `src/bully/config/skip.py`. Files matched by any pattern
//! short-circuit `HectorEngine::check` — no rules evaluated, no LLM dispatched.

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::Path;

/// Filename loaded from `$HOME` to add user-global skip globs.
pub const USER_GLOBAL_IGNORE_FILENAME: &str = ".hector-ignore";

/// Files we never want to lint — lockfiles, minified bundles, generated code,
/// and the usual build/dependency directories.
pub fn built_in_skip_globs() -> &'static [&'static str] {
    &[
        // Lockfiles
        "Cargo.lock",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "bun.lock",
        "poetry.lock",
        "Pipfile.lock",
        // Minified assets
        "*.min.js",
        "*.min.css",
        // Build / dependency directories
        "dist/**",
        "build/**",
        "__pycache__/**",
        "node_modules/**",
        "target/**",
        ".next/**",
        ".nuxt/**",
        // Generated markers
        "*.generated.*",
        "*.pb.go",
        "*.g.dart",
        "*.freezed.dart",
    ]
}

/// Right-anchored skip matcher. A bare pattern like `*.lock` matches at any
/// depth — same convention as [`crate::config::scope::ScopeMatcher`].
pub struct SkipMatcher {
    set: GlobSet,
}

impl SkipMatcher {
    /// Build a matcher from the built-in patterns plus any extras the caller
    /// provides (project `skip:` list, `~/.hector-ignore` entries, etc.).
    pub fn with_built_ins(extras: &[String]) -> Result<Self> {
        let mut b = GlobSetBuilder::new();
        for g in built_in_skip_globs() {
            add_glob(&mut b, g)?;
        }
        for g in extras {
            add_glob(&mut b, g)?;
        }
        Ok(Self { set: b.build()? })
    }

    pub fn matches<P: AsRef<Path>>(&self, path: P) -> bool {
        self.set.is_match(path.as_ref())
    }
}

fn add_glob(b: &mut GlobSetBuilder, raw: &str) -> Result<()> {
    let glob = Glob::new(raw).with_context(|| format!("invalid skip glob: {raw}"))?;
    b.add(glob);
    if !raw.contains('/') {
        let prefixed = format!("**/{raw}");
        let glob = Glob::new(&prefixed).with_context(|| format!("invalid skip glob: {prefixed}"))?;
        b.add(glob);
    }
    Ok(())
}

/// Parse the contents of `~/.hector-ignore` into a list of globs.
/// Blank lines and `#` comments are dropped; lines are trimmed.
pub fn parse_user_global_ignore(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .map(|s| s.to_string())
        .collect()
}
```

- [x] **Step 2: Run unit tests**

Run: `cargo test -p hector-core --lib config::skip`
Expected: all tests pass.

---

## Phase 2 — Config schema + extends merging

### Task 3: Add `skip: Vec<String>` to Config + extends union

**Files:**
- Modify: `crates/hector-core/src/config/types.rs` — add `skip` field
- Modify: `crates/hector-core/src/config/extends.rs` — union inherited `skip:` into local

- [x] **Step 1: Add the field to `Config`**

In `crates/hector-core/src/config/types.rs`, modify the `Config` struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: u32,
    #[serde(default)]
    pub llm: Option<LlmConfig>,
    #[serde(default)]
    pub extends: Vec<String>,
    #[serde(default)]
    pub trust: Option<TrustBlock>,
    #[serde(default)]
    pub skip: Vec<String>,
    pub rules: BTreeMap<String, Rule>,
}
```

- [x] **Step 2: Union skip lists across extends**

In `crates/hector-core/src/config/extends.rs`, modify `merge_inherited`:

```rust
fn merge_inherited(local: &mut Config, inherited: Config) {
    for (id, rule) in inherited.rules {
        local.rules.entry(id).or_insert(rule);
    }
    if local.llm.is_none() {
        local.llm = inherited.llm;
    }
    // Skip entries are additive — the union of every config in the extends
    // chain is what fires. Order doesn't matter (globs are unordered set
    // semantics), so we just append.
    for g in inherited.skip {
        if !local.skip.contains(&g) {
            local.skip.push(g);
        }
    }
    // trust block is per-config; never inherited.
}
```

- [x] **Step 3: Add an extends test**

Add a new test to `crates/hector-core/tests/extends.rs` (or follow the pattern there). First peek at the file to find the right helper-style:

```rust
// At end of crates/hector-core/tests/extends.rs:

#[test]
fn extends_unions_skip_globs_from_parent_and_child() {
    use hector_core::config::parse_file_with_extends;
    let dir = tempfile::tempdir().unwrap();
    let parent_path = dir.path().join("parent.yml");
    std::fs::write(
        &parent_path,
        "schema_version: 2\nskip:\n  - \"*.snap\"\nrules: {}\n",
    )
    .unwrap();
    let child_path = dir.path().join("child.yml");
    std::fs::write(
        &child_path,
        "schema_version: 2\nextends: [\"./parent.yml\"]\nskip:\n  - \"fixtures/**\"\nrules: {}\n",
    )
    .unwrap();
    let cfg = parse_file_with_extends(&child_path).expect("parse");
    assert!(cfg.skip.contains(&"*.snap".to_string()));
    assert!(cfg.skip.contains(&"fixtures/**".to_string()));
}
```

If the existing `extends.rs` test file uses a different helper API (e.g. inline YAML strings, helper functions), follow that convention rather than the snippet above.

- [x] **Step 4: Run the test**

Run: `cargo test -p hector-core --test extends`
Expected: pass. Existing extends tests continue to pass (the new field is `#[serde(default)]`).

- [x] **Step 5: Run the full hector-core test sweep**

Run: `cargo test -p hector-core`
Expected: all green. Adding a `#[serde(default)]` field is non-breaking for existing fixtures.

---

## Phase 3 — Wire `~/.hector-ignore` into `HectorEngine::load`

### Task 4: Build the effective skip list at load time

**Files:**
- Modify: `crates/hector-core/src/runner.rs` — load `~/.hector-ignore`, build `SkipMatcher`, store on `HectorEngine`

- [x] **Step 1: Add `skip: SkipMatcher` field to `HectorEngine`**

In `crates/hector-core/src/runner.rs`:

```rust
use crate::config::skip::{
    parse_user_global_ignore, SkipMatcher, USER_GLOBAL_IGNORE_FILENAME,
};

pub struct HectorEngine {
    config: Config,
    config_dir: PathBuf,
    llm: Option<Box<dyn crate::llm::LlmClient>>,
    skip: SkipMatcher,
}
```

- [x] **Step 2: Build the matcher at the bottom of `load_with`**

Replace the `Ok(Self { config, config_dir, llm })` block with:

```rust
        let mut skip_extras = config.skip.clone();
        if let Some(home) = home_dir() {
            let ignore_path = home.join(USER_GLOBAL_IGNORE_FILENAME);
            if let Ok(raw) = std::fs::read_to_string(&ignore_path) {
                skip_extras.extend(parse_user_global_ignore(&raw));
            }
        }
        let skip = SkipMatcher::with_built_ins(&skip_extras)?;

        Ok(Self {
            config,
            config_dir,
            llm,
            skip,
        })
    }
```

- [x] **Step 3: Add the `home_dir` helper**

Avoid pulling a new dep (`dirs`, `home`) for one path. At the top of `runner.rs` (or in a small private helper module), add:

```rust
/// Resolve the current user's home directory from environment variables.
/// Mirrors what `dirs::home_dir` does on Unix and Windows without the dep.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}
```

- [x] **Step 4: Compile**

Run: `cargo build -p hector-core`
Expected: compiles. The skip matcher is loaded but not yet consulted.

---

## Phase 4 — Short-circuit in `runner::check`

### Task 5: Bail out at the top of `check` if the file is skipped

**Files:**
- Modify: `crates/hector-core/src/runner.rs` — `HectorEngine::check`

- [x] **Step 1: Add the short-circuit at the top of `check`**

In `crates/hector-core/src/runner.rs::check`, after the path/content/diff destructure but before constructing `disable_map`:

```rust
        if self.skip.matches(&path) {
            let elapsed = start.elapsed().as_millis() as u64;
            let verdict = Verdict {
                schema_version: crate::verdict::SCHEMA_VERSION,
                hector_version: env!("CARGO_PKG_VERSION").to_string(),
                status: crate::verdict::Status::Pass,
                violations: vec![],
                passed_checks: vec![],
                elapsed_ms: elapsed,
            };
            let _ = crate::telemetry::append(
                &self.config_dir.join(".hector/log.jsonl"),
                &crate::telemetry::LogEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    kind: "skipped".into(),
                    file: path.display().to_string(),
                    rule_id: None,
                    status: "pass".into(),
                    elapsed_ms: elapsed,
                },
            );
            return Ok(verdict);
        }
```

Note: we construct the `Verdict` directly (instead of `Verdict::pass()`) so that `elapsed_ms` is non-zero — useful for telemetry baselines that look at total skip-overhead time.

- [x] **Step 2: Run a focused test to confirm nothing regressed**

Run: `cargo test -p hector-core --test runner`
Expected: pass. (The runner test suite doesn't currently use lockfile fixtures, so no behavior change for it.)

---

### Task 6: End-to-end test for the short-circuit

**Files:**
- Create: `crates/hector-core/tests/runner_skip.rs`

- [x] **Step 1: Write the test**

```rust
//! A2 — skip-pattern short-circuit at the top of HectorEngine::check.

use hector_core::runner::{CheckInput, HectorEngine};
use std::fs;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    fs::write(&path, body).unwrap();
    let trusted =
        hector_core::trust::write_trust_block(&fs::read_to_string(&path).unwrap()).unwrap();
    fs::write(&path, trusted).unwrap();
    path
}

#[test]
fn cargo_lock_is_skipped_with_default_config() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        // A semantic rule that *would* match Cargo.lock if it ran.
        // Without an LLM client, semantic dispatch would error — so the
        // assertion that we got a clean Pass proves the rule never ran.
        "schema_version: 2\n\
         rules:\n\
           silly:\n\
             description: \"any\"\n\
             engine: semantic\n\
             scope: [\"**/*.lock\"]\n\
             severity: error\n",
    );

    let engine = HectorEngine::load(&cfg).expect("load");
    let lockfile = dir.path().join("Cargo.lock");
    fs::write(&lockfile, "# generated\n").unwrap();
    let verdict = engine
        .check(CheckInput::File {
            path: lockfile.clone(),
            content: fs::read_to_string(&lockfile).unwrap(),
        })
        .expect("check");
    assert_eq!(verdict.status, hector_core::verdict::Status::Pass);
    assert!(verdict.violations.is_empty());
    assert!(verdict.passed_checks.is_empty(), "no rules should run");

    let log = fs::read_to_string(dir.path().join(".hector/log.jsonl")).expect("telemetry");
    assert!(
        log.contains("\"kind\":\"skipped\""),
        "expected a skipped telemetry record; log was:\n{log}"
    );
}

#[test]
fn project_skip_list_is_honored() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\n\
         skip:\n  - \"custom-ignore.txt\"\n\
         rules:\n\
           always-fire:\n\
             description: \"any\"\n\
             engine: script\n\
             scope: [\"*.txt\"]\n\
             severity: error\n\
             script: \"exit 1\"\n",
    );

    let engine = HectorEngine::load(&cfg).expect("load");
    let target = dir.path().join("custom-ignore.txt");
    fs::write(&target, "x\n").unwrap();
    let verdict = engine
        .check(CheckInput::File {
            path: target,
            content: "x\n".into(),
        })
        .expect("check");
    assert_eq!(verdict.status, hector_core::verdict::Status::Pass);
    assert!(verdict.violations.is_empty());
}

#[test]
fn user_global_ignore_is_honored() {
    let dir = tempdir().unwrap();
    let fake_home = dir.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();
    fs::write(fake_home.join(".hector-ignore"), "*.special\n").unwrap();

    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\n\
         rules:\n\
           always-fire:\n\
             description: \"any\"\n\
             engine: script\n\
             scope: [\"*.special\"]\n\
             severity: error\n\
             script: \"exit 1\"\n",
    );

    let target = dir.path().join("foo.special");
    fs::write(&target, "x\n").unwrap();

    // Override HOME for this load so parse_user_global_ignore reads our file.
    let prev_home = std::env::var_os("HOME");
    // SAFETY: tests run single-threaded by default within a #[test] fn, but
    // env mutation is racy across cargo test threads. Wrap in a global mutex
    // if you start seeing flakes; for now this single test does the override.
    // Test runs serially because we set HOME and then load, both inline.
    std::env::set_var("HOME", &fake_home);
    let engine = HectorEngine::load(&cfg).expect("load");
    if let Some(h) = prev_home {
        std::env::set_var("HOME", h);
    } else {
        std::env::remove_var("HOME");
    }

    let verdict = engine
        .check(CheckInput::File {
            path: target,
            content: "x\n".into(),
        })
        .expect("check");
    assert_eq!(verdict.status, hector_core::verdict::Status::Pass);
}
```

- [x] **Step 2: Run the new test file**

Run: `cargo test -p hector-core --test runner_skip`
Expected: all three tests pass.

If `user_global_ignore_is_honored` flakes due to test-thread `HOME` racing, gate it:
```rust
#[test]
#[ignore = "mutates global HOME — run with --ignored or under --test-threads=1"]
fn user_global_ignore_is_honored() { ... }
```

We accept the manual-flag trade-off for one test in 0.2; a full move to a `tracing`-style scoped env would be over-engineering.

---

## Phase 5 — CLI self-check + verification

### Task 7: CLI smoke test that real Cargo.lock is skipped

**Files:**
- Modify: `crates/hector-cli/tests/cli_check.rs` — add a test pointing at the workspace's actual `Cargo.lock`

- [x] **Step 1: Add the smoke test**

```rust
#[test]
fn check_skips_cargo_lock_with_default_config() {
    use std::fs;
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join(".hector.yml");
    fs::write(
        &cfg_path,
        "schema_version: 2\n\
         rules:\n\
           noop:\n\
             description: \"any\"\n\
             engine: script\n\
             scope: [\"*.lock\"]\n\
             severity: error\n\
             script: \"exit 1\"\n",
    )
    .unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg_path.to_str().unwrap()])
        .assert()
        .success();

    let lockfile = dir.path().join("Cargo.lock");
    fs::write(&lockfile, "# generated\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg_path.to_str().unwrap(),
            "--file",
            lockfile.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(0); // Pass — the script rule never runs.
}
```

- [x] **Step 2: Run the new CLI test**

Run: `cargo test -p hector-cli --test cli_check check_skips_cargo_lock_with_default_config`
Expected: pass.

---

### Task 8: Workspace verification

- [x] **Step 1: Format**

Run: `cargo fmt --all`
Expected: no diff (or trivial wrapping fixes).

- [x] **Step 2: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings. Watch for `clippy::ptr_arg` on the `&[String]` extras parameter — `Vec<String>` callers go through `&` which is fine.

- [x] **Step 3: Full test sweep**

Run: `cargo test --workspace`
Expected: green across all crates. Specifically:
- All existing `hector-core` tests still pass.
- All existing `hector-cli` tests still pass.
- The new `runner_skip` and CLI Cargo.lock tests pass.

- [x] **Step 4: Live self-check**

Run from the hector workspace root:
```bash
cargo build --release
./target/release/hector init   # if not already initialized
./target/release/hector trust
./target/release/hector check --file Cargo.lock --format json | jq .status
```
Expected: `"pass"`. (And no LLM call attempted, no rule evaluated.) If `hector init` doesn't exist or doesn't write a config, just hand-write a minimal `.hector.yml` with one rule that scopes `*.lock`.

---

## Test plan summary

| Layer | File | What it asserts |
|---|---|---|
| Unit (skip module) | `crates/hector-core/src/config/skip.rs` (`#[cfg(test)] mod tests`) | Built-in lists are complete; matcher honors right-anchored bare globs and recursive `dir/**` patterns; user globs are additive; `parse_user_global_ignore` strips blanks and `#` comments. |
| Unit (extends merge) | `crates/hector-core/tests/extends.rs` | Inherited `skip:` lists are unioned with local. |
| Integration (runner) | `crates/hector-core/tests/runner_skip.rs` | `Cargo.lock` short-circuits; project `skip:` honored; `~/.hector-ignore` honored. Skipped verdict has empty violations + empty passed_checks. Telemetry record written with `kind: "skipped"`. |
| CLI smoke | `crates/hector-cli/tests/cli_check.rs` | `hector check --file Cargo.lock` exits 0 with `status: "pass"` even when a script rule is scoped to `*.lock` (proves the rule never ran). |
| Manual self-check | (live) | `./target/release/hector check --file Cargo.lock` returns `pass` against the hector workspace itself. |

---

## Risk / rollback

- **Verdict-schema impact:** **none**. We deliberately fold "skipped" into `Status::Pass` with empty violations/passed_checks. No new enum variant; `SCHEMA_VERSION` stays at 1.
- **Telemetry-schema impact:** additive — new `kind: "skipped"` value within the existing flat `LogEntry` shape. D1 (typed telemetry) supersedes this; the upgrade path is captured in the D1 plan.
- **Config-schema impact:** additive — new top-level `skip: Vec<String>` (default empty). Configs without `skip:` are byte-identical to before; their trust fingerprint is unchanged. Configs that add `skip:` will need `hector trust` to re-acknowledge.
- **Trust-fingerprint impact:** **none** for users who don't add `skip:`. The fingerprint is computed over raw YAML, so absence of the key means the canonical form is unchanged.
- **Performance impact:** at `HectorEngine::load`, one filesystem read of `~/.hector-ignore` (silently skipped if absent), one `GlobSet::build` over ~20 patterns. Per-`check` call: one `GlobSet::is_match` against the file path before any rule iteration. Net: `check` on skipped files is dramatically *faster* (no rules, no LLM).
- **Behavioral risk:** A user with a project rule scoped to `*.lock` (intentional!) will be silently skipped under the default. Mitigation: document this in the changelog and in the `hector doctor` output (C1) once that ships. For 0.2, the right escape valve is configuring the rule scope to a more specific path that doesn't intersect built-ins, or — if there's real demand — add the deferred `skip.replace_builtins: true` flag in 0.3.
- **Rollback:** revert the commits for Tasks 2–7. The skip module can stay (`skip.rs`) without breaking anything since nothing references it.

---

## Self-review checklist (run before handing off)

1. **Spec coverage.** §A2 acceptance criteria:
   - [x] `.hector.yml` accepts a top-level `skip:` list → Task 3.
   - [x] Built-ins applied even if user list is empty → Task 4 (`SkipMatcher::with_built_ins(&[])` always seeds built-ins).
   - [x] `Cargo.lock` skipped by default → Task 6 (runner test) + Task 7 (CLI test) + Task 8 step 4 (live).
   - [x] `~/.hector-ignore` honored if present; absent is silent no-op → Task 4 + Task 6 third test.
   - [x] Skipped verdict has `status: Pass` + empty `violations`/`passed_checks` → Task 5 + Task 6 first test.
   - [x] Open question (Q2): `Status::Skipped` folded into `Pass` for 0.2 → ratified in decisions table; revisit before 0.3.
2. **No placeholders.** Every step shows the exact code or command. No "add appropriate validation" / "handle errors" — the only conditional handling is the documented HOME-flake fallback in Task 6.
3. **Type / signature consistency.** `SkipMatcher::with_built_ins(extras: &[String])`, `parse_user_global_ignore(raw: &str) -> Vec<String>`, `home_dir() -> Option<PathBuf>` are referenced consistently across tasks.
4. **Out-of-scope items deliberately deferred.** `Status::Skipped` enum variant → 0.3 verdict freeze decision. Per-rule `semantic_skipped` reasons → A3. Doctor surfacing of skip patterns → C1. `--skip-replace-builtins` flag → unless real demand emerges.

---

## Hand-off

After Task 8 is green: A2 is shipped. Next item in the 0.2.0 cohort is **A3 (diff pre-filter — `can_match_diff`)**; write `plans/2026-…-hector-a3-diff-prefilter.md` next. A3 only depends on the `engine: semantic` dispatch path and is independent of A2's runner-level short-circuit.
