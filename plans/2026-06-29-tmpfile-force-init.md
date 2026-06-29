# `$HECTOR_TMPFILE`, `--force`, stack-agnostic `init` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a write-event `$HECTOR_TMPFILE` (a hector-managed temp file holding proposed content for file-oriented tools), a scope-only `--force` test bypass on `hector check`, and make `hector init` stack-agnostic (drop Biome/ESLint/ruff templates).

**Architecture:** `$HECTOR_TMPFILE` is materialized in `runner.rs::run_one_check` (lazily, only when a check references the token; write-event only) and surfaced through a new `GateEnv.tmpfile` field that `engine/gate.rs::run_gate` exports; an RAII guard removes the file on every exit path. `--force` adds a `force: bool` to `CheckOptions`, read in `skip_reason` to suppress only the `out_of_scope` branch for named checks. `init` replaces its per-stack template assemblers with one constant universal baseline and deletes the now-dead `detect.rs`.

**Tech Stack:** Rust (workspace: `hector-core` lib + `hector-cli` bin `hector`), `clap` (CLI), `tempfile` (test dirs), `assert_cmd` (CLI e2e). No new dependencies.

**Design spec:** `specs/2026-06-29-hector-tmpfile-force-init-design.md`

## Global Constraints

- **No schema change.** `$HECTOR_TMPFILE` is additive to the ABI. Verdict stays `SCHEMA_VERSION = 5`; telemetry stays `5`. Do not change any verdict/telemetry shape.
- **ABI is a stability surface.** `$HECTOR_TMPFILE` is additive (new env var, only ever set, never required). Existing checks that never mention it are byte-for-byte unaffected.
- **Coverage:** every touched `crates/*/src/` file must hit ≥80% **region** coverage (`bash scripts/ci-coverage.sh`, cargo-llvm-cov; CI-enforced per file, **no exclusion mechanism**). Coverage tooling does not run locally (no llvm-tools-preview) — write the tests; CI verifies. Every new helper and every branch (Some/None ext, referenced/not, write/non-write, materialize ok/err) needs a test.
- **Cognitive complexity:** ≤15 per function (clippy `clippy.toml`, `#![warn(clippy::cognitive_complexity)]` at crate roots). Keep `run_one_check` simple by extracting `maybe_materialize_tmpfile`, `check_references_tmpfile`, `unique_tmp_name`. Refactor over `#[allow]`.
- **Lint/format:** `cargo clippy --all-targets -- -D warnings` and `cargo fmt` clean before every commit.
- **TDD:** every behavioral change starts with a failing test (use the test-writing skill).
- **`Cargo.lock` is gitignored** — never `git add` it.
- **Binary name is `hector`**, not `hector-cli`.
- **Exit-code contract** (`commands/check.rs`) unchanged: `0` pass, `1` config/load/usage error, `2` block, `3` internal. `--force` without `--check` is a usage error → exit `1`.
- **Trust:** unchanged. `init` still blesses the scaffolded config; read-only commands don't enforce trust.
- **After each task**, request code review from a separate agent before moving on (AGENTS.md rule).
- **Clean up** any `cargo build --release` / `cargo mutants` artifacts you create (`cargo clean -p <crate>`, remove scratch files); the cleanup-build-artifacts skill covers this.

---

## File Structure

**Modified (hector-core):**
- `crates/hector-core/src/engine/gate.rs` — `GateEnv` gains `tmpfile: Option<&Path>`; `run_gate` exports `HECTOR_TMPFILE`. (Task 1)
- `crates/hector-core/src/runner.rs` — `CheckOptions.force`; `skip_reason` honors force; `run_one_check` materializes the temp file via new private helpers + `TmpFileGuard`. (Tasks 2, 3)

**Modified (hector-cli):**
- `crates/hector-cli/src/cli.rs` — `Check` subcommand gains `--force`. (Task 3)
- `crates/hector-cli/src/commands/check.rs` — plumb `force`; validate `--force` requires `--check`. (Task 3)
- `crates/hector-cli/src/commands/init/mod.rs` — replace per-stack assemblers with one universal baseline; drop `Stack`/`detect_stack`/`build_config` args/`emit_*`. (Task 4)

**Deleted:**
- `crates/hector-cli/src/commands/init/detect.rs` — consumed only by config scaffolding (verified: no other consumer). (Task 4)

**Docs/skill:**
- `AGENTS.md` + `CLAUDE.md` — ABI paragraph (line 60) gains `$HECTOR_TMPFILE`. (Task 2)
- `adapters/shared/hector-config/SKILL.md` — ABI bullets gain `$HECTOR_TMPFILE` + a temp-file linter example; feeds `hector schema`. (Task 2)

**Tests:**
- `crates/hector-core/src/engine/gate.rs` (inline `tests`) — gate exports the var. (Task 1)
- `crates/hector-core/src/runner.rs` (inline `gate_dispatch_tests`) — materialization/cleanup/lazy/pre-commit + force. (Tasks 2, 3)
- `crates/hector-cli/tests/cli_e2e_gates.rs` — `--content` tmpfile path + `--force`. (Tasks 2, 3)
- `crates/hector-cli/tests/cli_init.rs` + `crates/hector-cli/src/commands/init/mod.rs` inline tests — universal baseline, no tool names. (Task 4)
- `crates/hector-cli/src/commands/schema.rs` + `crates/hector-cli/tests/cli_schema.rs` — guide mentions `$HECTOR_TMPFILE`. (Task 2)

---

## Task 1: `GateEnv.tmpfile` + `run_gate` exports `$HECTOR_TMPFILE`

**Files:**
- Modify: `crates/hector-core/src/engine/gate.rs` (struct `GateEnv` ~14-26; `run_gate` env block ~73-85; test helpers ~168-184)
- Modify: `crates/hector-core/src/runner.rs` (every `GateEnv { … }` construction: `run_one_check` ~496, `check_set` ~550, any test helper) — add `tmpfile: None`

**Interfaces:**
- Produces: `GateEnv { file: Option<&Path>, files: &[PathBuf], root: &Path, event: &str, tmpfile: Option<&'a Path> }`. When `tmpfile` is `Some(p)`, `run_gate` sets env `HECTOR_TMPFILE=<p>`; when `None`, the var is unset.

- [ ] **Step 1: Write the failing test** (append to `gate.rs` `mod tests`)

```rust
#[test]
fn tmpfile_env_is_set_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("x.txt");
    let tmp = dir.path().join("hector-tmp-1.txt");
    let env = GateEnv {
        file: Some(&f),
        files: &[],
        root: dir.path(),
        event: "write",
        tmpfile: Some(&tmp),
    };
    // Gate passes iff $HECTOR_TMPFILE equals the path we passed.
    let run = format!("test \"$HECTOR_TMPFILE\" = \"{}\"", tmp.display());
    assert!(matches!(run_gate(&run, &env, None, t()), GateOutcome::Pass));
}

#[test]
fn tmpfile_env_is_unset_when_none() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("x.txt");
    let env = GateEnv { file: Some(&f), files: &[], root: dir.path(), event: "write", tmpfile: None };
    // With the var unset, `test -n` on it is false → exit 1 → Block. Pass means it WAS set (bug).
    assert!(matches!(run_gate("test -n \"$HECTOR_TMPFILE\"", &env, None, t()), GateOutcome::Block { .. }));
}
```

- [ ] **Step 2: Run test to verify it fails (compile error — field missing)**

Run: `cargo test -p hector-core tmpfile_env`
Expected: FAIL — `GateEnv` has no field `tmpfile` / missing field in existing constructions.

- [ ] **Step 3: Add the field and export the var**

In `gate.rs`, add to `GateEnv`:

```rust
    /// Absolute path to a hector-materialized temp file holding the proposed
    /// content (`$HECTOR_TMPFILE`). `Some` only on `write` when the check
    /// references the token; `None` otherwise (var unset).
    pub tmpfile: Option<&'a Path>,
```

In `run_gate`, after the `if let Some(f) = env.file { cmd.env("HECTOR_FILE", f); }` block, add:

```rust
    if let Some(tf) = env.tmpfile {
        cmd.env("HECTOR_TMPFILE", tf);
    }
```

- [ ] **Step 4: Fix all other `GateEnv` constructions to compile**

Add `tmpfile: None,` to:
- the two test helpers in `gate.rs` (`env_for`, `env_with_files`),
- `runner.rs::run_one_check` (the `GateEnv { … }` ~496),
- `runner.rs::check_set` (the `GateEnv { … }` ~550),
- any `GateEnv { … }` in `runner.rs` tests (grep `rg -n "GateEnv \{" crates/`).

- [ ] **Step 5: Run tests to verify pass + workspace compiles**

Run: `cargo test -p hector-core tmpfile_env && cargo build`
Expected: PASS; workspace builds.

- [ ] **Step 6: Lint, format, commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add crates/hector-core/src/engine/gate.rs crates/hector-core/src/runner.rs
git commit -m "feat(core): add GateEnv.tmpfile; run_gate exports \$HECTOR_TMPFILE"
```

---

## Task 2: Materialize `$HECTOR_TMPFILE` in the runner (lazy, write-only, RAII cleanup) + docs

**Files:**
- Modify: `crates/hector-core/src/runner.rs` — new `TmpFileGuard`, `check_references_tmpfile`, `unique_tmp_name`, `maybe_materialize_tmpfile`; rewire `run_one_check` (~484-503)
- Modify: `AGENTS.md`, `CLAUDE.md` (ABI paragraph, line 60), `adapters/shared/hector-config/SKILL.md` (ABI bullets ~29-33, examples ~49-54)
- Test: `crates/hector-core/src/runner.rs` (`gate_dispatch_tests`), `crates/hector-cli/tests/cli_e2e_gates.rs`, `crates/hector-cli/src/commands/schema.rs`, `crates/hector-cli/tests/cli_schema.rs`

**Interfaces:**
- Consumes: `GateEnv.tmpfile` from Task 1; `Check::effective_steps()` (yields the check's steps, `run` folded in, each with `.run: String`).
- Produces (private to `runner.rs`):
  - `fn check_references_tmpfile(check: &Check) -> bool`
  - `fn unique_tmp_name(ext: Option<&str>) -> String`
  - `struct TmpFileGuard { path: PathBuf }` with `impl Drop` removing the file
  - `fn maybe_materialize_tmpfile(&self, check: &Check, abs: &Path, content: &str) -> Result<Option<TmpFileGuard>, std::io::Error>` — `Ok(None)` = not needed; `Ok(Some)` = created; `Err` = write failed
- Behavior: on `write`, if the check references `HECTOR_TMPFILE`, write `content` to `<dir of abs>/hector-tmp-<unique>[.<ext>]` (`<ext>` mirrors `abs`'s final extension), export its absolute path, remove it when the guard drops. Materialize failure → `CheckStatus::Error`.

- [ ] **Step 1: Write the failing tests** (append to `runner.rs` `gate_dispatch_tests`)

```rust
#[test]
fn tmpfile_materialized_with_content_ext_and_cleaned() {
    let dir = TempDir::new().unwrap();
    // Check copies $HECTOR_TMPFILE to a stable capture path, asserts the .rs ext, then passes.
    write_config(&dir,
        "checks:\n  cap:\n    files: \"**/*.rs\"\n    run: \"case \\\"$HECTOR_TMPFILE\\\" in *.rs) cat \\\"$HECTOR_TMPFILE\\\" > \\\"$HECTOR_ROOT/captured.txt\\\"; exit 0;; *) exit 2;; esac\"\n");
    let engine = load_with_event(&dir, "write");
    let path = dir.path().join("src").join("a.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "OLD").unwrap();
    let report = engine.check_with_explain(CheckInput::File {
        path: path.clone(),
        content: "PROPOSED-NEW".to_string(),
    }).unwrap();
    assert_eq!(report.verdict.status, Status::Pass);
    // The captured bytes are the PROPOSED content (not the OLD on-disk bytes).
    assert_eq!(std::fs::read_to_string(dir.path().join("captured.txt")).unwrap(), "PROPOSED-NEW");
    // The temp file is gone (cleanup), but its sibling source file remains.
    let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("hector-tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "temp file leaked: {leftovers:?}");
}

#[test]
fn tmpfile_not_created_when_unreferenced() {
    let dir = TempDir::new().unwrap();
    write_config(&dir, "checks:\n  g:\n    files: \"**/*.rs\"\n    run: \"! grep -q TODO\"\n");
    let engine = load_with_event(&dir, "write");
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fine").unwrap();
    let _ = engine.check_with_explain(CheckInput::File { path: path.clone(), content: "fine".into() }).unwrap();
    let any_tmp = std::fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().starts_with("hector-tmp-"));
    assert!(!any_tmp, "no temp file should exist for an unreferenced check");
}

#[test]
fn tmpfile_unset_on_pre_commit() {
    let dir = TempDir::new().unwrap();
    // On pre-commit the var must be empty even though the check references it.
    write_config(&dir, "checks:\n  pc:\n    files: \"**/*.rs\"\n    on: [pre-commit]\n    run: \"test -z \\\"$HECTOR_TMPFILE\\\"\"\n");
    let engine = load_with_event(&dir, "pre-commit");
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "x").unwrap();
    let verdict = engine.check_set(&[path]).unwrap();
    assert_eq!(verdict.status, Status::Pass, "HECTOR_TMPFILE must be unset on pre-commit");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p hector-core tmpfile_`
Expected: FAIL — `tmpfile_materialized_*` blocks/errors (no temp file yet); `tmpfile_unset_on_pre_commit` passes already (var never set). The materialization test is the red one.

- [ ] **Step 3: Add the helpers + guard** (in `runner.rs`, near `run_one_check`)

```rust
/// Removes its temp file on drop — covers normal return, error, timeout, and
/// panic-unwind. Only a SIGKILL of hector itself leaks the file.
struct TmpFileGuard {
    path: PathBuf,
}

impl Drop for TmpFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// True iff any of the check's steps reference the `$HECTOR_TMPFILE` token.
fn check_references_tmpfile(check: &Check) -> bool {
    check
        .effective_steps()
        .iter()
        .any(|s| s.run.contains("HECTOR_TMPFILE"))
}

/// A collision-resistant temp-file name mirroring `ext` (no `rng` dependency).
fn unique_tmp_name(ext: Option<&str>) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    match ext {
        Some(e) => format!("hector-tmp-{pid}-{n}-{nanos}.{e}"),
        None => format!("hector-tmp-{pid}-{n}-{nanos}"),
    }
}
```

Add the method to `impl HectorEngine` (near `run_one_check`):

```rust
    /// Materialize `$HECTOR_TMPFILE` for a `write` check that references it.
    /// `Ok(None)` = not needed; `Ok(Some)` = created (guard owns cleanup);
    /// `Err` = the write failed (caller surfaces it as an internal error so a
    /// tmpfile-dependent check never silently runs without its file).
    fn maybe_materialize_tmpfile(
        &self,
        check: &Check,
        abs: &Path,
        content: &str,
    ) -> std::io::Result<Option<TmpFileGuard>> {
        if self.options.event != "write" || !check_references_tmpfile(check) {
            return Ok(None);
        }
        let Some(parent) = abs.parent() else {
            return Ok(None);
        };
        let name = unique_tmp_name(abs.extension().and_then(|e| e.to_str()));
        let path = parent.join(name);
        std::fs::write(&path, content)?;
        Ok(Some(TmpFileGuard { path }))
    }
```

- [ ] **Step 4: Rewire `run_one_check`** (replace the body ~484-503)

```rust
    fn run_one_check(
        &self,
        check_id: &str,
        check: &Check,
        abs: &Path,
        match_path: &Path,
        content: &str,
    ) -> CheckStatus {
        if let Some(reason) = self.skip_reason(check_id, check, match_path, content) {
            return CheckStatus::Skipped(reason);
        }
        let tmp = match self.maybe_materialize_tmpfile(check, abs, content) {
            Ok(t) => t,
            Err(e) => {
                return CheckStatus::Error {
                    step: Some("<tmpfile>".to_string()),
                    reason: format!("tmpfile_write_failed:{e}"),
                    elapsed: 0,
                }
            }
        };
        let abs_buf = abs.to_path_buf();
        let env = GateEnv {
            file: Some(abs),
            files: std::slice::from_ref(&abs_buf),
            root: &self.config_dir,
            event: &self.options.event,
            tmpfile: tmp.as_ref().map(|g| g.path.as_path()),
        };
        self.run_steps(check, &env, Some(content.as_bytes()))
        // `tmp` drops here → temp file removed.
    }
```

(Field types confirmed at `runner.rs:95-98`: `CheckStatus::Error { step: Option<String>, reason: String, elapsed: u64 }` — hence `step: Some(...)`. `effective_steps()` returns an owned `Vec<Step>` (`config/types.rs:89`), so `.iter().any(...)` in `check_references_tmpfile` borrows the temporary within the expression — valid.)

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test -p hector-core tmpfile_`
Expected: PASS (all three).

- [ ] **Step 6: Add the materialize-failure test** (read-only parent dir → Err path)

```rust
#[test]
#[cfg(unix)]
fn tmpfile_write_failure_is_internal_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    write_config(&dir, "checks:\n  cap:\n    files: \"**/*.rs\"\n    run: \"cat \\\"$HECTOR_TMPFILE\\\"\"\n");
    let engine = load_with_event(&dir, "write");
    let sub = dir.path().join("ro");
    std::fs::create_dir(&sub).unwrap();
    let path = sub.join("a.rs");
    std::fs::write(&path, "x").unwrap();
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();
    let verdict = engine.check(CheckInput::File { path, content: "x".into() }).unwrap();
    // restore perms so TempDir cleanup works
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(verdict.status, Status::InternalError);
}
```

Run: `cargo test -p hector-core tmpfile_write_failure_is_internal_error`
Expected: PASS.

- [ ] **Step 7: Add the CLI `--content` e2e** (append to `cli_e2e_gates.rs`, mirror existing test style)

```rust
#[test]
fn content_flag_materializes_tmpfile() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".hector.yml"),
        "checks:\n  cap:\n    files: \"**/*.rs\"\n    run: \"case \\\"$HECTOR_TMPFILE\\\" in *.rs) cat \\\"$HECTOR_TMPFILE\\\" > \\\"$HECTOR_ROOT/cap.txt\\\"; exit 0;; *) exit 2;; esac\"\n").unwrap();
    let target = dir.path().join("a.rs");
    std::fs::write(&target, "OLD").unwrap();
    // bless trust (check enforces it)
    Command::cargo_bin("hector").unwrap().current_dir(dir.path())
        .args(["trust", "--config", ".hector.yml"]).assert().success();
    Command::cargo_bin("hector").unwrap().current_dir(dir.path())
        .args(["check", "--file", "a.rs", "--content", "-", "--config", ".hector.yml"])
        .write_stdin("NEWBYTES")
        .assert().success();
    assert_eq!(std::fs::read_to_string(dir.path().join("cap.txt")).unwrap(), "NEWBYTES");
}
```

(Match the file's existing imports/helpers — `use assert_cmd::Command;` etc. If the file has a `trust`+`check` helper, reuse it instead of the inline calls above.)

Run: `cargo test -p hector-cli --test cli_e2e_gates content_flag_materializes_tmpfile`
Expected: PASS.

- [ ] **Step 8: Document the new ABI var**

In **`AGENTS.md`** and **`CLAUDE.md`** (identical paragraph at line 60), append to the ABI sentence after the `$HECTOR_EVENT` clause:

```
, `$HECTOR_TMPFILE` (write-only, and only when the check's `run`/`steps` reference it: absolute path to a hector-materialized temp file — sibling of `$HECTOR_FILE`, same extension — holding the proposed content; auto-removed after the check)
```

In **`adapters/shared/hector-config/SKILL.md`**, add a bullet after the `$HECTOR_EVENT` bullet (~line 32):

```
- `$HECTOR_TMPFILE` — **write only**, set only when your `run` mentions it: an absolute path to a temp file holding the proposed content, placed beside `$HECTOR_FILE` with the same extension and auto-cleaned. Use it for tools that need a real file on disk (Biome, ESLint file-mode, `tsc`, ruff) instead of stdin. Unset on `pre-commit` (files are already on disk at `$HECTOR_FILES`).
```

And add an example after the existing linter example (~line 54):

```
**Wrap a file-oriented linter (temp file).** Tools that won't read stdin cleanly get a real path:

```yaml
biome-check:
  files: ["src/**/*.{ts,tsx,js,jsx}"]
  run: "npx @biomejs/biome check \"$HECTOR_TMPFILE\""
```
```

- [ ] **Step 9: Add the guide assertion**

In `crates/hector-cli/src/commands/schema.rs` (the `GUIDE` test ~line 52-53), add:

```rust
    assert!(strip_frontmatter(GUIDE).contains("$HECTOR_TMPFILE"));
```

In `crates/hector-cli/tests/cli_schema.rs` (after the `$HECTOR_FILE` assertion ~line 19), add:

```rust
    assert!(s.contains("$HECTOR_TMPFILE"), "guide must mention $HECTOR_TMPFILE:\n{s}");
```

Run: `cargo test -p hector-cli schema && cargo test -p hector-cli --test cli_schema`
Expected: PASS.

- [ ] **Step 10: Full check, lint, format, commit**

```bash
cargo test && cargo fmt && cargo clippy --all-targets -- -D warnings
git add crates/hector-core/src/runner.rs crates/hector-cli/tests/cli_e2e_gates.rs \
        crates/hector-cli/src/commands/schema.rs crates/hector-cli/tests/cli_schema.rs \
        AGENTS.md CLAUDE.md adapters/shared/hector-config/SKILL.md
git commit -m "feat(core): materialize \$HECTOR_TMPFILE on write events (lazy, auto-cleaned)"
```

---

## Task 3: `--force` — scope-only test bypass

**Files:**
- Modify: `crates/hector-core/src/runner.rs` — `CheckOptions.force` (~14-35), `skip_reason` (~432-452)
- Modify: `crates/hector-cli/src/cli.rs` — `Check` subcommand (~18-59)
- Modify: `crates/hector-cli/src/commands/check.rs` — `run` signature + options + validation (~10-54)
- Test: `crates/hector-core/src/runner.rs` (`gate_dispatch_tests`), `crates/hector-cli/tests/cli_e2e_gates.rs`

**Interfaces:**
- Consumes: existing `CheckOptions`, `skip_reason`, `commands/check::run`.
- Produces: `CheckOptions { …, force: bool }` (Default `false`); `hector check --force` (requires `--check`); `run(…, force: bool)`.

- [ ] **Step 1: Write the failing core test** (append to `gate_dispatch_tests`)

```rust
#[test]
fn force_runs_out_of_scope_named_check() {
    let dir = TempDir::new().unwrap();
    write_config(&dir, "checks:\n  only-src:\n    files: \"src/**/*.rs\"\n    run: \"! grep -q BAD\"\n");
    // File path is OUTSIDE the src/**/*.rs glob.
    let path = dir.path().join("fixtures").join("x.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "BAD").unwrap();
    let engine = HectorEngine::builder()
        .with_options(CheckOptions {
            checks: ["only-src".to_string()].into_iter().collect(),
            event: "write".to_string(),
            allow_external_paths: false,
            force: true,
        })
        .load(&dir.path().join(".hector.yml"))
        .unwrap();
    let report = engine.check_with_explain(CheckInput::File { path, content: "BAD".into() }).unwrap();
    // Without force it would be skipped out_of_scope → Pass. With force it fires → Block.
    assert_eq!(report.verdict.status, Status::Block);
}
```

- [ ] **Step 2: Run to verify it fails (compile error — no `force` field)**

Run: `cargo test -p hector-core force_runs_out_of_scope_named_check`
Expected: FAIL — `CheckOptions` has no field `force`.

- [ ] **Step 3: Add `force` to `CheckOptions` + `skip_reason`**

In the `CheckOptions` struct add:

```rust
    /// Suppress the `out_of_scope` skip for explicitly named (`checks`) ids, so
    /// an ad-hoc `--file` outside a check's glob still runs it. Scope-only:
    /// lifecycle and disable directives still apply.
    pub force: bool,
```

In `impl Default for CheckOptions`, add `force: false,`.

In `skip_reason`, replace the scope branch:

```rust
        if !self.check_matches_path(check_id, match_path) {
            let forced = self.options.force && self.options.checks.contains(check_id);
            if !forced {
                return Some("out_of_scope".to_string());
            }
        }
```

- [ ] **Step 4: Run the core test to verify pass; add the negative test**

Run: `cargo test -p hector-core force_runs_out_of_scope_named_check`
Expected: PASS.

Append the guard test (force must NOT bypass disable):

```rust
#[test]
fn force_does_not_bypass_disable_directive() {
    let dir = TempDir::new().unwrap();
    write_config(&dir, "checks:\n  only-src:\n    files: \"src/**/*.rs\"\n    run: \"! grep -q BAD\"\n");
    let path = dir.path().join("x.rs");
    std::fs::write(&path, "BAD").unwrap();
    let engine = HectorEngine::builder()
        .with_options(CheckOptions {
            checks: ["only-src".to_string()].into_iter().collect(),
            event: "write".to_string(),
            allow_external_paths: false,
            force: true,
        })
        .load(&dir.path().join(".hector.yml"))
        .unwrap();
    // Inline disable suppresses the check even under --force.
    let content = "BAD\n// hector-disable: only-src\n".to_string();
    let report = engine.check_with_explain(CheckInput::File { path, content }).unwrap();
    assert_eq!(report.verdict.status, Status::Pass);
}
```

Run: `cargo test -p hector-core force_does_not_bypass_disable_directive`
Expected: PASS.

- [ ] **Step 5: Fix other `CheckOptions { … }` literals to compile**

`commands/check.rs` (~29) and any test that builds `CheckOptions { … }` with explicit fields now need `force`. Grep `rg -n "CheckOptions \{" crates/` and add `force: …` to each (tests that don't care: `force: false`).

- [ ] **Step 6: Add the CLI flag**

In `cli.rs`, inside `Check { … }`, after `allow_external_paths` add:

```rust
        /// Run the named `--check` id(s) against `--file` even if the path is
        /// outside their `files` glob. Scope-only; requires `--check`.
        #[arg(long, default_value_t = false)]
        force: bool,
```

- [ ] **Step 7: Plumb + validate in `check.rs` and `main.rs`**

In `main.rs` (the `Command::Check { … }` arm, lines 13-33), add `force,` to the destructured fields and pass `force` as the final argument to `commands::check::run(…)`. Add `force: bool` as the last parameter of `check::run(...)`. At the top of `run`, before the trust gate, add:

```rust
    if force && checks.is_empty() {
        eprintln!("ERROR: --force requires at least one --check <id>");
        return Ok(1);
    }
```

Set `force` in the `CheckOptions` built at ~29:

```rust
    let options = CheckOptions {
        checks: HashSet::new(),
        event,
        allow_external_paths,
        force,
    };
```

(`checks` is still set via `set_check_filter` later; `options.checks` must be populated for `skip_reason`'s `contains` to see the named ids — verify `set_check_filter` updates `self.options.checks`. It does, per `set_check_filter` ~316. The force-without-check guard above runs before load, so this is consistent.)

- [ ] **Step 8: Add the CLI e2e** (append to `cli_e2e_gates.rs`)

```rust
#[test]
fn force_without_check_is_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".hector.yml"),
        "checks:\n  g:\n    files: \"src/**/*.rs\"\n    run: \"true\"\n").unwrap();
    Command::cargo_bin("hector").unwrap().current_dir(dir.path())
        .args(["check", "--file", "x.rs", "--force", "--config", ".hector.yml"])
        .assert().failure().code(1);
}
```

Run: `cargo test -p hector-cli --test cli_e2e_gates force_without_check_is_usage_error`
Expected: PASS.

- [ ] **Step 9: Full check, lint, format, commit**

```bash
cargo test && cargo fmt && cargo clippy --all-targets -- -D warnings
git add crates/hector-core/src/runner.rs crates/hector-cli/src/cli.rs \
        crates/hector-cli/src/commands/check.rs crates/hector-cli/tests/cli_e2e_gates.rs
git commit -m "feat(cli): add --force to bypass scope for named checks in ad-hoc testing"
```

---

## Task 4: Stack-agnostic `init`

**Files:**
- Modify: `crates/hector-cli/src/commands/init/mod.rs` — `scaffold_config` (~51-71), remove `Stack`/`detect_stack`/`build_config`/`emit_*`/`scope_list_with_default`, remove `detect::*` import + `mod detect;`, replace with `baseline_config()`; update inline tests (~350-375)
- Delete: `crates/hector-cli/src/commands/init/detect.rs`
- Test: `crates/hector-cli/tests/cli_init.rs` (~101-124, 203)

**Interfaces:**
- Consumes: nothing new.
- Produces: `fn baseline_config() -> String` (or a `const BASELINE: &str`) — the universal `.hector.yml` body, identical regardless of manifest.

- [ ] **Step 1: Write the failing test** (replace per-stack assertions in `cli_init.rs`)

```rust
#[test]
fn init_scaffolds_universal_baseline_regardless_of_stack() {
    for manifest in ["Cargo.toml", "package.json", "pyproject.toml", "none"] {
        let dir = tempfile::tempdir().unwrap();
        if manifest != "none" {
            std::fs::write(dir.path().join(manifest), "").unwrap();
        }
        Command::cargo_bin("hector").unwrap().current_dir(dir.path())
            .args(["init", "--no-hook"]).assert().success();
        let cfg = std::fs::read_to_string(dir.path().join(".hector.yml")).unwrap();
        assert!(cfg.contains("no-fixme:"), "{manifest}: missing no-fixme:\n{cfg}");
        assert!(cfg.contains("no-merge-markers:"), "{manifest}: missing no-merge-markers:\n{cfg}");
        assert!(cfg.contains("$HECTOR_TMPFILE"), "{manifest}: missing tmpfile example:\n{cfg}");
        // No toolchain-specific scaffolding.
        for tool in ["biome", "eslint", "ruff", "clippy", "no-unwrap", "console.log"] {
            assert!(!cfg.contains(tool), "{manifest}: must not scaffold `{tool}`:\n{cfg}");
        }
    }
}
```

(Use `--no-hook` so the test scaffolds the config without touching harness settings. If the existing init tests use a different invocation/helper, follow it.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p hector-cli --test cli_init init_scaffolds_universal_baseline_regardless_of_stack`
Expected: FAIL — current output contains `biome`/`no-unwrap`/etc. for detected stacks and lacks `no-merge-markers`/`$HECTOR_TMPFILE`.

- [ ] **Step 3: Add the baseline + simplify `scaffold_config`**

Add near the top of `mod.rs`:

```rust
/// The universal, stack-agnostic starter config. Two checks that work on any
/// file (read proposed content from stdin), plus commented examples for the
/// two real authoring patterns. hector knows nothing about any toolchain.
const BASELINE: &str = r#"checks:
  no-fixme:
    files: ["*"]
    run: "! grep -nE 'FIXME'"
  no-merge-markers:
    files: ["*"]
    run: "! grep -nE '^(<<<<<<< |=======$|>>>>>>> )'"

# --- examples (uncomment and adapt) ---
#
# Wrap a file-oriented linter via the materialized temp file. On a write event
# $HECTOR_TMPFILE holds the proposed content with the right extension, beside
# the real file, and is removed after the check:
#
#   biome-check:
#     files: ["src/**/*.{ts,tsx,js,jsx}"]
#     run: "npx @biomejs/biome check \"$HECTOR_TMPFILE\""
#
# A stdin/grep check (proposed content arrives on stdin; nonzero blocks):
#
#   no-console-log:
#     files: ["src/**/*.{ts,js}"]
#     run: "! grep -nE 'console\\.log\\('"
"#;
```

Replace `scaffold_config`'s detection/build block (lines 57-61) with:

```rust
    let body = BASELINE.to_string();
```

- [ ] **Step 4: Remove the dead per-stack code**

Delete from `mod.rs`: `enum Stack`, `fn detect_stack`, `fn build_config`, `emit_rust_gates`, `emit_node_gates`, `emit_python_gates`, `emit_generic_gates`, `emit_linter_gate`, `scope_list_with_default` (and any helper used only by them). Remove the `use detect::{…}` line and the `mod detect;` declaration. Delete `crates/hector-cli/src/commands/init/detect.rs`. Remove or rewrite the inline `mod.rs` tests at ~350-375 that assert per-stack `$HECTOR_FILE` usage — replace with:

```rust
    #[test]
    fn baseline_has_universal_checks_and_tmpfile_example() {
        assert!(BASELINE.contains("no-fixme:"));
        assert!(BASELINE.contains("no-merge-markers:"));
        assert!(BASELINE.contains("$HECTOR_TMPFILE"));
        for tool in ["biome", "eslint", "ruff"] {
            assert!(!BASELINE.contains(tool));
        }
    }
```

- [ ] **Step 5: Run init tests + confirm scaffolded config validates**

Run: `cargo test -p hector-cli --test cli_init && cargo build`
Expected: PASS; no unused-import/dead-code warnings.

Add a validation test (a scaffolded config must parse) if `cli_init.rs` doesn't already cover it:

```rust
#[test]
fn scaffolded_baseline_validates() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("hector").unwrap().current_dir(dir.path())
        .args(["init", "--no-hook"]).assert().success();
    Command::cargo_bin("hector").unwrap().current_dir(dir.path())
        .args(["validate", "--config", ".hector.yml"]).assert().success();
}
```

Run: `cargo test -p hector-cli --test cli_init scaffolded_baseline_validates`
Expected: PASS.

- [ ] **Step 6: Full check, lint, format, commit**

```bash
cargo test && cargo fmt && cargo clippy --all-targets -- -D warnings
git add crates/hector-cli/src/commands/init/mod.rs crates/hector-cli/tests/cli_init.rs
git rm crates/hector-cli/src/commands/init/detect.rs
git commit -m "feat(cli): make init stack-agnostic; drop toolchain-specific templates"
```

---

## Task 5: Final verification

- [ ] **Step 1: Full workspace test + lint + format**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all PASS, clippy clean, fmt clean.

- [ ] **Step 2: Confirm no leftover stack detection references**

Run: `rg -n "detect_stack|emit_rust_gates|emit_node_gates|emit_python_gates|Stack::" crates/`
Expected: no matches.

- [ ] **Step 3: Confirm the ABI doc surfaces the new var everywhere**

Run: `rg -l "HECTOR_TMPFILE" AGENTS.md CLAUDE.md adapters/shared/hector-config/SKILL.md`
Expected: all three listed.

- [ ] **Step 4: Clean up build artifacts** (cleanup-build-artifacts skill)

Remove any release binaries / scratch files this work produced; do not touch the iterating `target/`.

---

## Self-Review

**Spec coverage:**
- §2 `$HECTOR_TMPFILE` (placement, lazy, write-only, RAII, CLI testing, failure-as-internal, docs) → Tasks 1, 2. ✓
- §2.7 known limitation (synthetic name) → documented in SKILL.md bullet (Task 2, Step 8). ✓
- §3 `--force` (scope-only, requires `--check`, doesn't bypass disable/lifecycle, exit 1 without `--check`) → Task 3. ✓
- §4 stack-agnostic init (remove emit_*, two universal checks, commented examples, delete detect.rs, harness onboarding untouched) → Task 4. ✓
- §5 ABI docs (AGENTS/CLAUDE/SKILL + schema guide test) → Task 2, Steps 8-9. ✓
- §6 testing (materialization, cleanup, lazy-skip, pre-commit unset, CLI --content, force fire/usage-error/disable) → Tasks 2, 3. ✓
- §7 out-of-scope items → intentionally not implemented. ✓

**Placeholder scan:** No TBD/TODO; every code step shows real code; every test shows assertions. ✓

**Type consistency:** `GateEnv.tmpfile: Option<&Path>` (Task 1) consumed in Task 2 via `tmp.as_ref().map(|g| g.path.as_path())`. `CheckOptions.force: bool` (Task 3) read in `skip_reason`. `maybe_materialize_tmpfile -> io::Result<Option<TmpFileGuard>>`, `TmpFileGuard.path: PathBuf`, `check_references_tmpfile`/`unique_tmp_name` signatures match call sites. `CheckStatus::Error { step: String, reason: String, elapsed: u64 }` matches existing usage in `run_steps`. ✓
