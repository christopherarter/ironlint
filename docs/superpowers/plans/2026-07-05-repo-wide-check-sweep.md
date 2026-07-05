# Repo-Wide Check Sweep Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bare `ironlint check` (no `--file`, no `--diff`) sweeps every policy-scoped file under the config root and returns one aggregate verdict — with batched (set-mode) dispatch as the primary performance lever.

**Architecture:** The sweep is CLI-side orchestration over existing core APIs; `ironlint-core` is untouched. A gitignore-aware walker (the `ignore` crate) produces a sorted candidate file list rooted at the config directory. Checks are then split into two disjoint classes by lifecycle: checks whose `on:` includes `pre-commit` run **once each** over the full matched set via the existing `IronLintEngine::check_set` (`$IRONLINT_FILES` populated, stdin empty) — one process spawn per check instead of one per file; write-only checks run per file with on-disk content on stdin, reusing the exact fold `--diff` mode already implements. The two phases' outcomes merge into a single `Verdict` via `Verdict::from_outcomes`.

**Tech Stack:** Rust, clap 4, `ignore` 0.4 (new CLI-only dependency), `assert_cmd`/`tempfile` for e2e tests.

## Global Constraints

- Exit-code contract is locked: `0` pass, `1` config/load/usage error, `2` block, `3` internal error, `4` untrusted. The sweep introduces no new codes.
- Verdict JSON shape is locked (`verdict::SCHEMA_VERSION = 5`). The sweep only reuses `Verdict::from_outcomes` — no shape change, no version bump.
- The check ABI is locked: no new env vars, no new `$IRONLINT_EVENT` values. The sweep reuses `write` (per-file phase) and `pre-commit` (batch phase) exactly as specified.
- Trust is enforced at the CLI `check` layer before any dispatch (existing code path, unchanged); `IronLintEngine::load` stays pure.
- Every touched `crates/*/src/` file must meet ≥80% region coverage (`bash scripts/ci-coverage.sh`).
- Cognitive complexity ≤15 per function (`clippy.toml`); refactor rather than annotate.
- `cargo clippy --all-targets -- -D warnings` must stay clean; `cargo fmt` before every commit.
- `Cargo.lock` is committed; regenerate via plain `cargo build` after the dependency addition and commit it with the same task.
- Scope semantics are intentional: a bare pattern without `/` (e.g. `*.py`) matches at any depth. Do not "fix" this.

## Non-Goals (explicitly deferred)

- **Parallel per-file dispatch (rayon):** land batching first, measure, then decide. Not in this plan.
- **Verdict caching:** unsound without per-check purity opt-in; separate design if ever needed.
- **Positional path arguments** (`ironlint check docs/`): follow-up once the walker exists.
- **`--event` composed with a bare sweep:** rejected with a usage error in v1 (the sweep derives each check's lifecycle from its `on:` list). Relaxing this later is additive.
- **Daemon / persistent workers:** rejected permanently; conflicts with the spawn-`sh -c`-read-exit-code model.

## Performance Rationale (why batch-first)

Sweep cost is invocation count × tool startup. Fork+exec of `sh` is ~1–5ms, but a check wrapping `npx biome` pays ~300ms+ per invocation and `tsc --noEmit` is whole-program — per-file dispatch of such checks is minutes of pure startup overhead that parallelism cannot buy back. The pre-commit lifecycle already defines the batched shape (one invocation, `$IRONLINT_FILES`, empty stdin) and `check_set` already implements it, so the sweep gets its O(checks) fast path with zero new ABI. Guidance for config authors becomes: give expensive checks a `pre-commit` lifecycle and read `$IRONLINT_FILES`.

## Dispatch Rules (the contract this plan implements)

1. Bare `ironlint check` = sweep. `--file`/`--diff` behavior is unchanged.
2. Walk root = the directory containing the resolved config. Regular files only; hidden entries skipped; `.gitignore`/`.ignore` honored even outside a git repo (`require_git(false)`). Sorted for deterministic output.
3. Class split (disjoint, each check runs exactly once per sweep):
   - `on:` contains `pre-commit` (including `[write, pre-commit]`) → **batched**: one `check_set` invocation per check over the full walked list (engine scope-matches internally).
   - otherwise (write-only) → **per-file**: on-disk content on stdin, `--diff`-mode semantics, including skip-with-warning for unreadable/non-UTF-8 files.
4. `--check` filters apply to both classes. `--require-match`, `--format`, `--explain` (per-file rows only) work as elsewhere. `--event` and `--force` with a bare sweep are usage errors (exit 1). `--content` already requires `--file` via clap.
5. Verdicts from both phases merge via `Verdict::from_outcomes`; existing `exit_code`/`emit` logic applies unchanged.

## File Structure

- `crates/ironlint-cli/Cargo.toml` — add `ignore = "0.4"` (CLI-only dep, like `ratatui`).
- `crates/ironlint-cli/src/commands/sweep.rs` — **new.** Walker (`walk_files`), lifecycle classifier (`classify_checks` + `SweepClasses`), sweep orchestration (`run`). Unit tests for walker + classifier live here.
- `crates/ironlint-cli/src/commands/check.rs` — extract shared per-file fold (`check_files_individually` + `FoldedOutcomes`); widen helper visibility to `pub(crate)`; rewire `run()`'s dispatch match; `event` becomes `Option<String>`.
- `crates/ironlint-cli/src/commands/mod.rs` — register `pub mod sweep;`.
- `crates/ironlint-cli/src/cli.rs` — `event` arg drops its default (becomes `Option<String>`); doc comments updated.
- `crates/ironlint-cli/src/main.rs` — no edit needed (destructures and passes `event` through; the type change propagates).
- `crates/ironlint-cli/tests/cli_check_sweep.rs` — **new.** All sweep e2e tests.
- `crates/ironlint-cli/tests/cli_usage_errors_exit_1.rs`, `tests/cli_event_validation.rs`, `tests/cli_diff_skips_unreadable_file.rs` — expectation updates where behavior intentionally changed (called out inside tasks).

Existing pieces reused as-is (do not modify): `trust::check_trust` gate, `validate_check_filter`, `emit_error`, `Verdict::from_outcomes`, `IronLintEngine::{check_set, check_with_explain, check_matches_path, checks, set_check_filter}`, `tests/common/mod.rs::blessed_store`.

---

### Task 1: Walker + classifier module (`sweep.rs`)

**Files:**
- Modify: `crates/ironlint-cli/Cargo.toml` (add `ignore = "0.4"` under `[dependencies]`)
- Modify: `crates/ironlint-cli/src/commands/mod.rs` (add `pub mod sweep;`)
- Create: `crates/ironlint-cli/src/commands/sweep.rs`

**Interfaces:**
- Consumes: `ironlint_core::config::{Check, Lifecycle}` (re-exported via `pub use types::*`; `Lifecycle::{Write, PreCommit}` derives `PartialEq`).
- Produces (used by Tasks 3–4):
  - `pub(crate) struct SweepClasses { pub per_file: HashSet<String>, pub batched: HashSet<String> }`
  - `pub(crate) fn classify_checks(checks: &BTreeMap<String, Check>, user_filter: &HashSet<String>) -> SweepClasses`
  - `pub(crate) fn walk_files(root: &Path) -> anyhow::Result<Vec<PathBuf>>`

- [ ] **Step 1: Add the dependency**

In `crates/ironlint-cli/Cargo.toml`, under `[dependencies]` after the `ratatui` entry:

```toml
# Repo-wide sweep (`ironlint check` with no --file/--diff): gitignore-aware
# directory walker — the same rule engine ripgrep uses.
ignore = "0.4"
```

- [ ] **Step 2: Write the failing unit tests**

Create `crates/ironlint-cli/src/commands/sweep.rs` with ONLY the test module first (types/fns referenced don't exist yet), and add `pub mod sweep;` to `crates/ironlint-cli/src/commands/mod.rs` (alphabetical: between `show_resolved_config` and `trust`):

```rust
//! Bare `ironlint check` — the repo-wide sweep.

#[cfg(test)]
mod tests {
    use super::*;
    use ironlint_core::config::{Check, Lifecycle};
    use std::collections::{BTreeMap, HashSet};

    fn check_on(on: Vec<Lifecycle>) -> Check {
        Check {
            files: vec!["*.md".to_string()],
            run: Some("exit 0".to_string()),
            steps: None,
            on,
            name: None,
        }
    }

    #[test]
    fn classify_splits_write_only_from_batchable() {
        let mut checks = BTreeMap::new();
        checks.insert("w".to_string(), check_on(vec![Lifecycle::Write]));
        checks.insert("p".to_string(), check_on(vec![Lifecycle::PreCommit]));
        checks.insert(
            "both".to_string(),
            check_on(vec![Lifecycle::Write, Lifecycle::PreCommit]),
        );
        let classes = classify_checks(&checks, &HashSet::new());
        assert_eq!(classes.per_file, HashSet::from(["w".to_string()]));
        assert_eq!(
            classes.batched,
            HashSet::from(["p".to_string(), "both".to_string()])
        );
    }

    #[test]
    fn classify_honors_user_filter() {
        let mut checks = BTreeMap::new();
        checks.insert("w".to_string(), check_on(vec![Lifecycle::Write]));
        checks.insert("p".to_string(), check_on(vec![Lifecycle::PreCommit]));
        let filter = HashSet::from(["p".to_string()]);
        let classes = classify_checks(&checks, &filter);
        assert!(classes.per_file.is_empty());
        assert_eq!(classes.batched, HashSet::from(["p".to_string()]));
    }

    #[test]
    fn walk_collects_sorted_files_skipping_hidden_and_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("b/.hidden")).unwrap();
        std::fs::create_dir_all(root.join("vendor")).unwrap();
        std::fs::write(root.join("z.md"), "z").unwrap();
        std::fs::write(root.join("b/a.md"), "a").unwrap();
        std::fs::write(root.join("b/.hidden/skip.md"), "s").unwrap();
        // `.ignore` (not `.gitignore`) so the rule holds without a git repo,
        // though require_git(false) makes .gitignore work here too.
        std::fs::write(root.join(".ignore"), "vendor/\n").unwrap();
        std::fs::write(root.join("vendor/skip.md"), "s").unwrap();

        let files = walk_files(root).unwrap();
        let canon = root.canonicalize().unwrap();
        assert_eq!(files, vec![canon.join("b/a.md"), canon.join("z.md")]);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p ironlint-cli sweep`
Expected: COMPILE ERROR — `classify_checks`, `walk_files`, `SweepClasses` not found.

- [ ] **Step 4: Implement the module**

Prepend to `sweep.rs` (above the test module):

```rust
//! Bare `ironlint check` — the repo-wide sweep.
//!
//! Dispatch model (each check runs exactly once per sweep, keyed by
//! lifecycle):
//!   - checks whose `on:` includes `pre-commit` are BATCHED: one `check_set`
//!     invocation per check over the full walked set ($IRONLINT_FILES
//!     populated, stdin empty) — one process spawn per check, not per file;
//!   - write-only checks run per file with on-disk content on stdin,
//!     exactly like `--diff` mode.
//! A dual-lifecycle check (`on: [write, pre-commit]`) is batched only, so it
//! is never double-run.

use anyhow::Result;
use ironlint_core::config::{Check, Lifecycle};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Which sweep phase each check id belongs to. The split is disjoint.
pub(crate) struct SweepClasses {
    pub per_file: HashSet<String>,
    pub batched: HashSet<String>,
}

/// Split the resolved check map into sweep phases, honoring the user's
/// `--check` filter (empty filter = all checks).
pub(crate) fn classify_checks(
    checks: &BTreeMap<String, Check>,
    user_filter: &HashSet<String>,
) -> SweepClasses {
    let mut per_file = HashSet::new();
    let mut batched = HashSet::new();
    for (id, check) in checks {
        if !user_filter.is_empty() && !user_filter.contains(id) {
            continue;
        }
        if check.on.contains(&Lifecycle::PreCommit) {
            batched.insert(id.clone());
        } else {
            per_file.insert(id.clone());
        }
    }
    SweepClasses { per_file, batched }
}

/// Walk `root` collecting the sweep's candidate files: regular files only,
/// hidden entries skipped, `.gitignore`/`.ignore` honored even outside a git
/// repo (`require_git(false)` — the sweep, not git, defines the working set).
/// Paths are relativized to the process cwd when possible so verdict output
/// stays repo-relative, and sorted so output and CI diffs are deterministic.
pub(crate) fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let root = root.canonicalize()?;
    let cwd = std::env::current_dir()?.canonicalize()?;
    let mut files: Vec<PathBuf> = ignore::WalkBuilder::new(&root)
        .require_git(false)
        .build()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| {
            let p = e.into_path();
            p.strip_prefix(&cwd).map(Path::to_path_buf).unwrap_or(p)
        })
        .collect();
    files.sort();
    Ok(files)
}
```

Note the walker test asserts absolute paths: the tempdir is outside the test process's cwd, so `strip_prefix` falls through to the absolute form. The e2e tests in Task 3 cover the relativized form (CLI cwd = project root).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ironlint-cli sweep`
Expected: PASS (3 tests).

- [ ] **Step 6: Lint, format, lock**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt && cargo build`
Expected: clean; `Cargo.lock` gains the `ignore` graph.

- [ ] **Step 7: Commit**

```bash
git add crates/ironlint-cli/Cargo.toml crates/ironlint-cli/src/commands/mod.rs crates/ironlint-cli/src/commands/sweep.rs Cargo.lock
git commit -m "feat(sweep): walker + lifecycle classifier for repo-wide check"
```

---

### Task 2: Extract the shared per-file fold from `run_diff`

Behavior-preserving refactor; the existing diff e2e suite is the safety net. No new tests.

**Files:**
- Modify: `crates/ironlint-cli/src/commands/check.rs:174-304` (`run_diff`, `SkipReason`, `read_changed_file`) and `:331-395` (`print_explain`, `exit_code`, `emit` visibility)
- Test (existing, may need one string assertion update): `crates/ironlint-cli/tests/cli_diff_skips_unreadable_file.rs`

**Interfaces:**
- Consumes: `IronLintEngine::check_with_explain(CheckInput::File { path, content }) -> Result<CheckReport>` (unchanged core API).
- Produces (used by Task 3–4's `sweep::run`):
  - `pub(crate) struct FoldedOutcomes { pub blocks: Vec<ironlint_core::verdict::Block>, pub errors: Vec<ironlint_core::verdict::GateError>, pub passed: Vec<String>, pub explains: Vec<CheckExplain>, pub elapsed_ms: u64 }`
  - `pub(crate) fn check_files_individually(engine: &IronLintEngine, paths: &[PathBuf]) -> Result<FoldedOutcomes>`
  - `pub(crate)` visibility on `emit`, `exit_code`, `print_explain` (unchanged signatures: `emit(v: &Verdict, format: OutputFormat) -> Result<()>`, `exit_code(v: &Verdict, require_match: bool) -> i32`, `print_explain(rows: &[CheckExplain])`).

- [ ] **Step 1: Add `FoldedOutcomes` + `check_files_individually` to check.rs**

Insert after `run_file` (before the current `run_diff`):

```rust
/// Aggregate of per-file check runs, ready to fold into one Verdict.
pub(crate) struct FoldedOutcomes {
    pub blocks: Vec<ironlint_core::verdict::Block>,
    pub errors: Vec<ironlint_core::verdict::GateError>,
    pub passed: Vec<String>,
    pub explains: Vec<CheckExplain>,
    pub elapsed_ms: u64,
}

/// Run `paths` one file at a time through the engine (write-lifecycle
/// semantics: on-disk content on stdin), folding the per-file verdicts.
/// A file we can't read (missing, permissions, non-UTF-8) is a SKIP, not a
/// hard error: fabricating empty content would run every check against ""
/// and let a real violation pass vacuously, but aborting the whole batch
/// would hide a real Block in a sibling file. Record the skip, warn loudly,
/// and move on. Extracted from `run_diff` so the bare-sweep path reuses the
/// identical fold.
pub(crate) fn check_files_individually(
    engine: &IronLintEngine,
    paths: &[PathBuf],
) -> Result<FoldedOutcomes> {
    let mut out = FoldedOutcomes {
        blocks: Vec::new(),
        errors: Vec::new(),
        passed: Vec::new(),
        explains: Vec::new(),
        elapsed_ms: 0,
    };
    for path in paths {
        let content = match read_changed_file(path) {
            Ok(c) => c,
            Err(reason) => {
                let label = reason.label();
                eprintln!(
                    "WARNING: skipping file {} ({label}): {reason}",
                    path.display()
                );
                out.explains.push(CheckExplain {
                    check_id: path.display().to_string(),
                    outcome: ExplainOutcome::Skipped {
                        reason: label.to_string(),
                    },
                });
                continue;
            }
        };
        let r = engine.check_with_explain(CheckInput::File {
            path: path.clone(),
            content,
        })?;
        out.elapsed_ms = out.elapsed_ms.saturating_add(r.verdict.elapsed_ms);
        out.blocks.extend(r.verdict.blocks);
        out.errors.extend(r.verdict.errors);
        out.passed.extend(r.verdict.passed);
        out.explains.extend(r.explain);
    }
    Ok(out)
}
```

- [ ] **Step 2: Rewire `run_diff` onto the helper**

Replace the entire body of `run_diff` (keep its doc comment, signature, and the pre-commit `check_set` branch):

```rust
fn run_diff(
    engine: &IronLintEngine,
    diff: &Path,
    format: OutputFormat,
    explain: bool,
    require_match: bool,
) -> Result<i32> {
    let unified = std::fs::read_to_string(diff)?;
    let changed = ironlint_core::diff::parser::parse_unified(&unified)?;
    if changed.is_empty() {
        return Ok(emit_error(format, "no changed files in diff", 1));
    }
    let non_deleted: Vec<PathBuf> = changed
        .iter()
        .filter(|f| f.op != ironlint_core::diff::ChangeOp::Deleted)
        .map(|f| f.path.clone())
        .collect();

    // Pre-commit: run each check once over the entire changed set.
    if engine.event() == "pre-commit" {
        let verdict = engine.check_set(&non_deleted)?;
        emit(&verdict, format)?;
        return Ok(exit_code(&verdict, require_match));
    }

    // Write (and any future per-file event): loop once per changed file.
    let folded = match check_files_individually(engine, &non_deleted) {
        Ok(f) => f,
        Err(e) => return Ok(emit_error(format, &format!("{e:#}"), 1)),
    };
    let verdict = Verdict::from_outcomes(
        folded.blocks,
        folded.errors,
        folded.passed,
        folded.elapsed_ms,
    );
    if explain {
        print_explain(&folded.explains);
    }
    emit(&verdict, format)?;
    Ok(exit_code(&verdict, require_match))
}
```

- [ ] **Step 3: Widen visibility**

Change `fn emit`, `fn exit_code`, `fn print_explain`, `fn read_changed_file`, and `enum SkipReason` from private to `pub(crate)` (Task 3–4's `sweep.rs` consumes `emit`/`exit_code`/`print_explain`; `read_changed_file`/`SkipReason` stay in check.rs but the compiler will flag if `pub(crate)` is unneeded — keep them private if only `check_files_individually` uses them).

- [ ] **Step 4: Run the diff suites**

Run: `cargo test -p ironlint-cli --test cli_diff_skips_unreadable_file --test cli_diff_read_error`
Expected: PASS. If an assertion matches the old warning phrase `skipping changed file`, update that assertion to the new phrase `skipping file` (the only intentional output change in this task).

- [ ] **Step 5: Full workspace tests + lint**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt`
Expected: PASS, clean.

- [ ] **Step 6: Commit**

```bash
git add crates/ironlint-cli/src/commands/check.rs crates/ironlint-cli/tests/cli_diff_skips_unreadable_file.rs
git commit -m "refactor(check): extract shared per-file fold for sweep reuse"
```

---

### Task 3: Wire the bare-sweep arm (per-file phase) + `--event` becomes optional

**Files:**
- Modify: `crates/ironlint-cli/src/cli.rs:44-49` (event arg)
- Modify: `crates/ironlint-cli/src/commands/check.rs:63-141` (`run` signature + dispatch match)
- Modify: `crates/ironlint-cli/src/commands/sweep.rs` (add `run`)
- Create: `crates/ironlint-cli/tests/cli_check_sweep.rs`
- Possibly modify: `crates/ironlint-cli/tests/cli_usage_errors_exit_1.rs`, `crates/ironlint-cli/tests/cli_event_validation.rs` (expectation updates, Step 6)

**Interfaces:**
- Consumes: Task 1's `walk_files`/`classify_checks`/`SweepClasses`; Task 2's `check_files_individually`/`FoldedOutcomes`/`emit`/`exit_code`/`print_explain`; `IronLintEngine::{set_check_filter, check_matches_path, checks}`.
- Produces: `pub(crate) fn run(engine: &mut IronLintEngine, config: &Path, user_checks: &HashSet<String>, format: OutputFormat, explain: bool, require_match: bool) -> Result<i32>` in `sweep.rs` (Task 4 extends this signature with `allow_external_paths: bool`).

- [ ] **Step 1: Write the failing e2e tests**

Create `crates/ironlint-cli/tests/cli_check_sweep.rs`:

```rust
//! E2E: bare `ironlint check` sweeps the repo (no --file / --diff).

mod common;

use assert_cmd::Command;
use common::blessed_store;
use std::fs;
use tempfile::TempDir;

fn project_with_config(yaml: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(".ironlint.yml"), yaml).unwrap();
    dir
}

fn ironlint(project: &TempDir, xdg: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.current_dir(project.path())
        .env("XDG_CONFIG_HOME", xdg.path());
    cmd
}

const GREP_CHECK: &str =
    "checks:\n  no-forbidden:\n    files: \"*.md\"\n    run: '! grep -n FORBIDDEN'\n";

#[test]
fn bare_check_sweeps_write_only_checks_across_nested_dirs() {
    let project = project_with_config(GREP_CHECK);
    fs::create_dir_all(project.path().join("docs/deep")).unwrap();
    fs::write(project.path().join("clean.md"), "all good\n").unwrap();
    fs::write(
        project.path().join("docs/deep/dirty.md"),
        "has FORBIDDEN word\n",
    )
    .unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    let assert = ironlint(&project, &xdg).arg("check").assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("no-forbidden"), "stderr: {stderr}");
    assert!(stderr.contains("docs/deep/dirty.md"), "stderr: {stderr}");
    // The clean file must not appear as a block.
    assert!(!stderr.contains("clean.md"), "stderr: {stderr}");
}

#[test]
fn bare_check_on_clean_repo_exits_zero() {
    let project = project_with_config(GREP_CHECK);
    fs::write(project.path().join("clean.md"), "all good\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    let assert = ironlint(&project, &xdg).arg("check").assert().code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(stdout.contains("pass"), "stdout: {stdout}");
}

#[test]
fn bare_check_rejects_explicit_event() {
    let project = project_with_config(GREP_CHECK);
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    let assert = ironlint(&project, &xdg)
        .args(["check", "--event", "write"])
        .assert()
        .code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("--event requires --file or --diff"),
        "stderr: {stderr}"
    );
}

#[test]
fn bare_check_rejects_force() {
    let project = project_with_config(GREP_CHECK);
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    let assert = ironlint(&project, &xdg)
        .args(["check", "--force", "--check", "no-forbidden"])
        .assert()
        .code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("--force requires --file"), "stderr: {stderr}");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p ironlint-cli --test cli_check_sweep`
Expected: FAIL — bare `check` currently exits 1 with `provide exactly one of --file or --diff`, so the first two tests fail on exit code; the `--event` test fails on message content.

- [ ] **Step 3: Make `--event` optional in cli.rs**

Replace the event arg (cli.rs:41-49):

```rust
        /// What triggered this check, surfaced to checks as $IRONLINT_EVENT.
        /// Defaults to `write` for `--file`/`--diff`. Not valid with a bare
        /// repo-wide sweep, which derives each check's lifecycle from its
        /// `on:` list. Restricted to the two ABI values; an unknown value is
        /// rejected at the arg layer so typos never reach `$IRONLINT_EVENT`.
        #[arg(
            long,
            value_parser = clap::builder::PossibleValuesParser::new(["write", "pre-commit"])
        )]
        event: Option<String>,
```

Also update the `Check` subcommand's doc comment (cli.rs:17): `/// Run the pipeline against a file, a diff, or — with neither — sweep the repo.`

- [ ] **Step 4: Rewire `check::run`**

In `check.rs`, change the signature's `event: String` to `event: Option<String>`, then inside `run` (replacing lines 117-140):

```rust
    let event_explicit = event.is_some();
    let event = event.unwrap_or_else(|| "write".to_string());
    let options = CheckOptions {
        checks: HashSet::new(),
        event,
        allow_external_paths,
        force,
    };
    let mut engine = match IronLintEngine::builder().with_options(options).load(config) {
        Ok(e) => e,
        Err(e) => return Ok(emit_error(format, &format!("{e:#}"), 1)),
    };
    if let Some(code) = validate_check_filter(&engine, &checks, format) {
        return Ok(code);
    }
    let check_filter: HashSet<String> = checks.into_iter().collect();
    engine.set_check_filter(check_filter.clone());

    match (file, diff) {
        (Some(f), None) => run_file(&engine, f, content, format, explain, require_match),
        (None, Some(d)) => run_diff(&engine, &d, format, explain, require_match),
        (Some(_), Some(_)) => Ok(emit_error(
            format,
            "provide exactly one of --file or --diff",
            1,
        )),
        (None, None) => {
            // Bare `check` = repo-wide sweep. The sweep derives each check's
            // lifecycle from its `on:` list, so a caller-chosen event is a
            // contradiction, and `--force` (scope bypass for one file) has
            // no meaning against a walked set.
            if event_explicit {
                return Ok(emit_error(
                    format,
                    "--event requires --file or --diff (a bare sweep runs each check's own lifecycle)",
                    1,
                ));
            }
            if force {
                return Ok(emit_error(format, "--force requires --file", 1));
            }
            crate::commands::sweep::run(
                &mut engine,
                config,
                &check_filter,
                format,
                explain,
                require_match,
            )
        }
    }
```

`main.rs` needs no edit — it destructures `event` and passes it through; the type change propagates.

- [ ] **Step 5: Implement `sweep::run` (per-file phase only in this task)**

Append to `sweep.rs` (above the test module), with these additional imports at the top: `use crate::cli::OutputFormat;`, `use crate::commands::check::{check_files_individually, emit, exit_code, print_explain};`, `use crate::commands::error_report::emit_error;`, `use ironlint_core::runner::IronLintEngine;`, `use ironlint_core::verdict::Verdict;`.

```rust
/// Bare-`check` orchestration. `engine` arrives loaded with event `write`,
/// trust-gated, and with the user's `--check` filter already validated.
pub(crate) fn run(
    engine: &mut IronLintEngine,
    config: &Path,
    user_checks: &HashSet<String>,
    format: OutputFormat,
    explain: bool,
    require_match: bool,
) -> Result<i32> {
    let parent = config.parent().unwrap_or_else(|| Path::new("."));
    let root = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let files = walk_files(root)?;
    let classes = classify_checks(engine.checks(), user_checks);

    let mut blocks = Vec::new();
    let mut errors = Vec::new();
    let mut passed = Vec::new();
    let mut explains = Vec::new();
    let mut elapsed: u64 = 0;

    // Phase 1 — write-only checks, one invocation per matching file.
    if !classes.per_file.is_empty() {
        engine.set_check_filter(classes.per_file.clone());
        // Prune to files at least one phase-1 check scopes to; without this,
        // every walked file would produce a no-op engine call and a
        // telemetry row.
        let scoped: Vec<PathBuf> = files
            .iter()
            .filter(|f| {
                classes
                    .per_file
                    .iter()
                    .any(|id| engine.check_matches_path(id, f))
            })
            .cloned()
            .collect();
        match check_files_individually(engine, &scoped) {
            Ok(folded) => {
                blocks.extend(folded.blocks);
                errors.extend(folded.errors);
                passed.extend(folded.passed);
                explains.extend(folded.explains);
                elapsed = elapsed.saturating_add(folded.elapsed_ms);
            }
            Err(e) => return Ok(emit_error(format, &format!("{e:#}"), 1)),
        }
    }

    let verdict = Verdict::from_outcomes(blocks, errors, passed, elapsed);
    if explain {
        print_explain(&explains);
    }
    emit(&verdict, format)?;
    Ok(exit_code(&verdict, require_match))
}
```

(Phase 2 — the batched `check_set` dispatch — lands in Task 4; a config whose checks are all batch-class produces an honest empty verdict until then, and Task 4's tests pin the real behavior.)

- [ ] **Step 6: Run the new suite + reconcile changed expectations**

Run: `cargo test -p ironlint-cli --test cli_check_sweep`
Expected: PASS (4 tests).

Run: `cargo test -p ironlint-cli --test cli_usage_errors_exit_1 --test cli_event_validation`
Expected: any test asserting that bare `check` exits 1 with `provide exactly one of --file or --diff` is now obsolete — delete that case (the sweep e2e supersedes it). The `--file`+`--diff` case and invalid `--event` value rejection must still pass unchanged.

- [ ] **Step 7: Full tests + lint + commit**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt`
Expected: PASS, clean.

```bash
git add crates/ironlint-cli/src/cli.rs crates/ironlint-cli/src/commands/check.rs crates/ironlint-cli/src/commands/sweep.rs crates/ironlint-cli/tests/cli_check_sweep.rs crates/ironlint-cli/tests/cli_usage_errors_exit_1.rs crates/ironlint-cli/tests/cli_event_validation.rs
git commit -m "feat(sweep): bare 'ironlint check' sweeps write-only checks repo-wide"
```

---

### Task 4: Batched phase — one `check_set` run per pre-commit-capable check

**Files:**
- Modify: `crates/ironlint-cli/src/commands/sweep.rs` (`run` gains the batch phase + `allow_external_paths` param)
- Modify: `crates/ironlint-cli/src/commands/check.rs` (sweep call site passes `allow_external_paths`)
- Test: `crates/ironlint-cli/tests/cli_check_sweep.rs` (three new tests)

**Interfaces:**
- Consumes: `IronLintEngine::check_set(&[PathBuf]) -> Result<Verdict>` (engine filters by lifecycle + scope internally; `Block.file` is `null` in set mode); `CheckOptions { checks: HashSet<String>, event: String, allow_external_paths: bool, force: bool }`.
- Produces: final signature `pub(crate) fn run(engine: &mut IronLintEngine, config: &Path, user_checks: &HashSet<String>, format: OutputFormat, explain: bool, require_match: bool, allow_external_paths: bool) -> Result<i32>`.

- [ ] **Step 1: Write the failing e2e tests**

Append to `cli_check_sweep.rs`:

```rust
#[test]
fn pre_commit_check_runs_once_over_the_matched_set() {
    let project = project_with_config(concat!(
        "checks:\n",
        "  set-check:\n",
        "    files: \"*.md\"\n",
        "    on: [pre-commit]\n",
        "    run: |\n",
        "      echo INVOKED >> invocations.txt\n",
        "      printf '%s\\n' \"$IRONLINT_FILES\" >> seen.txt\n",
        "      exit 0\n",
    ));
    fs::write(project.path().join("a.md"), "x\n").unwrap();
    fs::write(project.path().join("b.md"), "y\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    ironlint(&project, &xdg).arg("check").assert().code(0);

    let invocations =
        fs::read_to_string(project.path().join("invocations.txt")).unwrap();
    assert_eq!(
        invocations.lines().count(),
        1,
        "expected exactly one batched invocation, got: {invocations}"
    );
    let seen = fs::read_to_string(project.path().join("seen.txt")).unwrap();
    assert!(seen.contains("a.md"), "seen: {seen}");
    assert!(seen.contains("b.md"), "seen: {seen}");
    assert_eq!(
        seen.lines().count(),
        2,
        "expected both files in one $IRONLINT_FILES, got: {seen}"
    );
}

#[test]
fn dual_lifecycle_check_is_batched_not_double_run() {
    let project = project_with_config(concat!(
        "checks:\n",
        "  dual:\n",
        "    files: \"*.md\"\n",
        "    on: [write, pre-commit]\n",
        "    run: 'echo INVOKED >> invocations.txt; exit 0'\n",
    ));
    fs::write(project.path().join("a.md"), "x\n").unwrap();
    fs::write(project.path().join("b.md"), "y\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    ironlint(&project, &xdg).arg("check").assert().code(0);

    let invocations =
        fs::read_to_string(project.path().join("invocations.txt")).unwrap();
    assert_eq!(
        invocations.lines().count(),
        1,
        "a dual-lifecycle check must run exactly once per sweep, got: {invocations}"
    );
}

#[test]
fn batched_check_block_reaches_the_sweep_verdict() {
    let project = project_with_config(
        "checks:\n  always-block:\n    files: \"*.md\"\n    on: [pre-commit]\n    run: 'echo set-level violation; exit 1'\n",
    );
    fs::write(project.path().join("a.md"), "x\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    let assert = ironlint(&project, &xdg).arg("check").assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("always-block"), "stderr: {stderr}");
    assert!(stderr.contains("set-level violation"), "stderr: {stderr}");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p ironlint-cli --test cli_check_sweep`
Expected: the three new tests FAIL — batch-class checks currently never run in a sweep (`invocations.txt` missing / exit 0 instead of 2).

- [ ] **Step 3: Add the batch phase to `sweep::run`**

Add `allow_external_paths: bool` as the final parameter of `sweep::run`, add `use ironlint_core::runner::CheckOptions;` to the imports, and insert this block between Phase 1 and the `Verdict::from_outcomes` fold:

```rust
    // Phase 2 — batch-class checks: ONE invocation per check over the full
    // walked set (the engine scope-filters per check). This is the sweep's
    // primary performance lever: O(checks) process spawns instead of
    // O(files x checks) for every check that can run in set mode. A second
    // engine is loaded because the event is fixed at load time; config
    // parse + matcher build is microseconds against process spawns, and the
    // trust gate already ran for this config path in `check::run`.
    if !classes.batched.is_empty() {
        let options = CheckOptions {
            checks: classes.batched.clone(),
            event: "pre-commit".to_string(),
            allow_external_paths,
            force: false,
        };
        let batch_engine = match IronLintEngine::builder().with_options(options).load(config) {
            Ok(e) => e,
            Err(e) => return Ok(emit_error(format, &format!("{e:#}"), 1)),
        };
        match batch_engine.check_set(&files) {
            Ok(v) => {
                elapsed = elapsed.saturating_add(v.elapsed_ms);
                blocks.extend(v.blocks);
                errors.extend(v.errors);
                passed.extend(v.passed);
            }
            Err(e) => return Ok(emit_error(format, &format!("{e:#}"), 1)),
        }
    }
```

Update the call site in `check.rs` to pass `allow_external_paths` as the last argument.

If clippy's cognitive-complexity lint (cap 15) trips on `run` after this addition, extract each phase into its own helper (`run_per_file_phase`, `run_batched_phase`) returning `(Vec<Block>, Vec<GateError>, Vec<String>, u64)` folded by the caller — refactor, do not annotate.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p ironlint-cli --test cli_check_sweep`
Expected: PASS (7 tests).

- [ ] **Step 5: Full tests + lint + commit**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt`
Expected: PASS, clean.

```bash
git add crates/ironlint-cli/src/commands/sweep.rs crates/ironlint-cli/src/commands/check.rs crates/ironlint-cli/tests/cli_check_sweep.rs
git commit -m "feat(sweep): batch pre-commit-capable checks — one check_set run per check"
```

---

### Task 5: Edge-case e2e — ignore rules, filters, skips, trust

**Files:**
- Test: `crates/ironlint-cli/tests/cli_check_sweep.rs` (six new tests)
- Modify (only if a test exposes a defect): `crates/ironlint-cli/src/commands/sweep.rs`

**Interfaces:**
- Consumes: everything landed in Tasks 1–4; `tests/common/mod.rs::blessed_store`.
- Produces: nothing new — this task pins behavior.

- [ ] **Step 1: Write the tests**

Append to `cli_check_sweep.rs`:

```rust
#[test]
fn sweep_honors_gitignore_without_a_git_repo() {
    let project = project_with_config(GREP_CHECK);
    fs::write(project.path().join(".gitignore"), "vendor/\n").unwrap();
    fs::create_dir_all(project.path().join("vendor")).unwrap();
    fs::write(
        project.path().join("vendor/dirty.md"),
        "has FORBIDDEN word\n",
    )
    .unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    ironlint(&project, &xdg).arg("check").assert().code(0);
}

#[test]
fn sweep_skips_hidden_directories() {
    let project = project_with_config(GREP_CHECK);
    fs::create_dir_all(project.path().join(".secrets")).unwrap();
    fs::write(
        project.path().join(".secrets/dirty.md"),
        "has FORBIDDEN word\n",
    )
    .unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    ironlint(&project, &xdg).arg("check").assert().code(0);
}

#[test]
fn sweep_warns_and_skips_non_utf8_but_still_blocks_siblings() {
    let project = project_with_config(GREP_CHECK);
    fs::write(project.path().join("binary.md"), [0xFF, 0xFE, 0x00, 0x01]).unwrap();
    fs::write(project.path().join("dirty.md"), "has FORBIDDEN word\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    let assert = ironlint(&project, &xdg).arg("check").assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("WARNING") && stderr.contains("binary.md"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("dirty.md"), "stderr: {stderr}");
}

#[test]
fn sweep_check_filter_limits_to_named_check() {
    let project = project_with_config(concat!(
        "checks:\n",
        "  no-forbidden:\n",
        "    files: \"*.md\"\n",
        "    run: '! grep -n FORBIDDEN'\n",
        "  always-block:\n",
        "    files: \"*.txt\"\n",
        "    run: 'exit 1'\n",
    ));
    fs::write(project.path().join("clean.md"), "all good\n").unwrap();
    fs::write(project.path().join("note.txt"), "anything\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    // Only no-forbidden runs; always-block's guaranteed violation is filtered out.
    ironlint(&project, &xdg)
        .args(["check", "--check", "no-forbidden"])
        .assert()
        .code(0);
}

#[test]
fn sweep_require_match_flags_a_scope_that_matches_nothing() {
    let project = project_with_config(
        "checks:\n  ghost:\n    files: \"*.nomatch\"\n    run: 'exit 1'\n",
    );
    fs::write(project.path().join("a.md"), "x\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    ironlint(&project, &xdg).arg("check").assert().code(0);
    ironlint(&project, &xdg)
        .args(["check", "--require-match"])
        .assert()
        .code(2);
}

#[test]
fn sweep_fails_closed_on_untrusted_config() {
    let project = project_with_config(GREP_CHECK);
    fs::write(project.path().join("a.md"), "x\n").unwrap();
    // Fresh, empty trust store: the config was never blessed.
    let empty_xdg = tempfile::tempdir().unwrap();

    ironlint(&project, &empty_xdg).arg("check").assert().code(4);
}
```

- [ ] **Step 2: Run them**

Run: `cargo test -p ironlint-cli --test cli_check_sweep`
Expected: PASS on Tasks 1–4's implementation. Any failure here is a real defect in the sweep — fix it in `sweep.rs` (walker flags, class filter plumbing) with the failing test as the guide; do not weaken the test.

- [ ] **Step 3: Full tests + lint + commit**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt`
Expected: PASS, clean.

```bash
git add crates/ironlint-cli/tests/cli_check_sweep.rs crates/ironlint-cli/src/commands/sweep.rs
git commit -m "test(sweep): pin ignore rules, filters, non-utf8 skip, trust fail-closed"
```

---

### Task 6: Docs, coverage gate, cleanup

**Files:**
- Modify: `CLAUDE.md` (Commands section + CLI description)
- Modify: `crates/ironlint-cli/src/cli.rs` (only if Task 3's doc comments missed anything `--help` should say)

**Interfaces:** none — documentation and gates.

- [ ] **Step 1: Update CLAUDE.md**

In the `## Commands` fenced block, add after the `cargo build --release` line:

```bash
./target/release/ironlint check              # bare = repo-wide sweep (batched where checks allow)
```

In the `## What this is` paragraph, extend the CLI ships list: change `` CLI ships `check`, `` to `` CLI ships `check` (with `--file`, `--diff`, or bare for a repo-wide sweep), ``.

- [ ] **Step 2: Run the coverage gate**

Run: `bash scripts/ci-coverage.sh`
Expected: every touched file ≥80% region coverage. If `sweep.rs` falls short, add the e2e below: a batch check whose command hits exit 127 (InternalError → exit 3) exercises the batch phase's error-aggregation fold, which the happy-path tests don't touch. The `Err` arms on the second engine load (config unreadable *after* the trust gate passed) are not reproducible in an e2e and may stay uncovered — the 80% threshold absorbs them; do not chase 100%.

```rust
#[test]
fn batched_check_internal_error_exits_three() {
    let project = project_with_config(
        "checks:\n  broken:\n    files: \"*.md\"\n    on: [pre-commit]\n    run: 'definitely-not-a-real-binary-xyz'\n",
    );
    fs::write(project.path().join("a.md"), "x\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    ironlint(&project, &xdg).arg("check").assert().code(3);
}
```

- [ ] **Step 3: Final gates**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test && bash scripts/ci-coverage.sh`
Expected: all clean/green.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md crates/ironlint-cli/tests/cli_check_sweep.rs
git commit -m "docs(sweep): document bare-check repo sweep; cover internal-error arm"
```

---

## Post-Plan Notes for the Reviewer

- **Why two engines?** `CheckOptions.event` is fixed at `IronLintEngine::load`; the per-file phase needs `write`, the batch phase `pre-commit`. Loading twice costs a YAML parse + matcher build (microseconds) and keeps `runner.rs` untouched. The alternative — a core API for per-call events — is a bigger surface change for zero user-visible gain.
- **Why is the dual-lifecycle check batched rather than per-file?** Running it both ways would double-report every violation; batching is the cheaper single run and matches the check author's declared ability to handle set mode. The inverse choice (per-file wins) would forfeit the performance lever for exactly the checks that opted into it.
- **Inline disable directives** are a write-lifecycle feature (scanned from per-file content). They apply in the sweep's per-file phase and — by existing `check_set` design — not in the batch phase. This mirrors `--diff --event pre-commit` behavior today; not a new inconsistency.
- **Telemetry:** per-file phase logs one record per file (as `--diff` does); batch phase logs one set-level record with `set_size` (existing `check_set` behavior). No schema change.
- **`walk_files` returns cwd-relative paths when the CLI runs at the repo root** (the normal case), falling back to absolute when cwd is elsewhere (e.g. `--config` pointing at another tree). Engine containment checks (`resolve_input_path`, `allow_external_paths`) are unaffected: walked files are by construction inside the config root.
