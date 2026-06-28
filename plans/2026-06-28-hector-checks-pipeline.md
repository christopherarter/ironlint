# Hector 0.4 — Checks Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reposition hector as "a local CI for agents" — rename `gates`→`checks`, flip the verdict to nonzero-blocks, add fail-fast `steps`, and add `write`/`pre-commit` lifecycles with `$HECTOR_FILES`.

**Architecture:** A check is `files` + (`run` xor `steps`) + optional `on`/`name`. The runner matches a touched file, and for each lifecycle in `on:` runs the check's steps fail-fast; a step exiting `1`–`125` blocks. `write` runs per-file with content on stdin; `pre-commit` runs once over the changed set via `$HECTOR_FILES`. Phases land in compile-dependency order: the vocabulary rename is the keystone, then grammar, engine, runner, CLI, dogfood/docs.

**Tech Stack:** Rust (workspace: `hector-core` lib + `hector-cli` bin `hector`), `serde`/`serde_yaml`, `globset`, `wait_timeout`, `assert_cmd` for CLI e2e.

**Design spec:** `specs/2026-06-28-hector-checks-pipeline-design.md`

## Global Constraints

- **Coverage:** every touched `crates/*/src/` file must hit ≥80% region coverage (`bash scripts/ci-coverage.sh`, cargo-llvm-cov; CI-enforced per file). Coverage tooling does not run locally (no llvm-tools-preview) — write the tests; CI verifies.
- **Cognitive complexity:** ≤15 per function (clippy, `clippy.toml`). Refactor over `#[allow]`.
- **Lint/format:** `cargo clippy --all-targets -- -D warnings` and `cargo fmt` clean before every commit.
- **TDD:** every behavioral change starts with a failing test. Mechanical renames are covered by the existing suite staying green.
- **`Cargo.lock` is gitignored** — never `git add` it.
- **Binary name is `hector`**, not `hector-cli`.
- **Outer exit codes are LOCKED:** `0` pass · `1` load/config error · `2` block · `3` internal. Do not change `check.rs`'s `exit_code()` mapping.
- **Verdict JSON is a locked surface:** `verdict::SCHEMA_VERSION` bumps `4`→`5`; telemetry `SCHEMA_VERSION` bumps `3`→`4` (versioned independently).
- **After each phase**, request code review from a separate agent before moving on.
- **Clean up** any `cargo build --release` / `cargo mutants` artifacts you create (`cargo clean -p <crate>`).

---

## File Structure

**Modified (hector-core):**
- `config/types.rs` — `Gate`→`Check` (+ `run: Option<String>`, `steps`, `on`, `name`), new `Step`, `Lifecycle`, `Check::effective_steps()`.
- `config/parser.rs` — `legacy_marker` gains `gates`; `parse_str` validates run-xor-steps + per-step empty-run.
- `config/mod.rs` — re-export `Check`, `Step`, `Lifecycle`.
- `engine/gate.rs` — `classify()` flip; `GateEnv` gains `files` + `file: Option`; sets `$HECTOR_FILES`.
- `runner.rs` — steps fail-fast loop; lifecycle dispatch (per-file vs run-once); `on:` filter; thread `event` to telemetry.
- `verdict.rs` — schema 5; `gate`→`check` JSON key; nullable `step`/`file`.
- `telemetry.rs` — schema 4; `gate`→`check`; `step`; `event`.

**Modified (hector-cli):**
- `cli.rs` — `--gate`→`--check`; `--event` drops `edit`.
- `commands/check.rs` — `gate`→`check` vocabulary; `--diff --event pre-commit` run-once wiring.
- `commands/explain.rs`, `commands/show_resolved_config.rs` — `gate`→`check` output.

**Modified (repo + tests):**
- `.hector.yml` — `gates:`→`checks:` (keystone), then `! grep` idiom (dogfood phase).
- `crates/hector-cli/tests/*.rs` — fixture YAML + assertions.
- `scripts/ci-dogfood.sh`, README, `hector-config` skill — vocabulary + DX.

**Untouched (regression guards):** `tests/fixtures/valid_v2.hector.yml`, `tests/fixtures/with_extends/*` (legacy-rejection fixtures), `config/scope.rs`, `config/extends.rs`, `disable.rs` mechanism (comments only).

---

# Phase 1 — The vocabulary rename (keystone)

Atomic rename `gates`→`checks`, `Gate`→`Check`, `.gate`→`.check`, plus the verdict/telemetry schema bumps. **Behavior is identical** (exit 2 still blocks) except JSON key names and schema numbers. The whole workspace must compile and `cargo test` must pass at the end — the rename cannot be partially green.

### Task 1.1: Rename the config types and the dogfood/fixture YAML

**Files:**
- Modify: `crates/hector-core/src/config/types.rs:4-49`
- Modify: `crates/hector-core/src/config/parser.rs:50-63` (legacy_marker) + tests
- Modify: `crates/hector-core/src/config/mod.rs`
- Modify: `.hector.yml` (repo root)
- Test: existing config tests (update fixture YAML `gates:`→`checks:`)

**Interfaces:**
- Produces: `Config { extends, execution, checks: BTreeMap<String, Check> }`; `Check { files: Vec<String>, run: String }` (structurally identical to old `Gate`, just renamed — `run`/`steps` split is Phase 2).

- [ ] **Step 1: Rename in `types.rs`.** `struct Gate` → `struct Check`; `Config.gates: BTreeMap<String, Gate>` → `checks: BTreeMap<String, Check>`. Keep `#[serde(deny_unknown_fields)]` and `files_one_or_many`. Update the 5 unit tests (L78-113): every `"gates:\n"` YAML literal → `"checks:\n"`, `cfg.gates` → `cfg.checks`.

- [ ] **Step 2: Add `gates` to the legacy marker.** In `parser.rs` `legacy_marker` (L50), prepend `"gates"` to the key loop and return a curated message:

```rust
fn legacy_marker(input: &str) -> Option<(&'static str, &'static str)> {
    let value: serde_yaml::Value = serde_yaml::from_str(input).ok()?;
    let map = value.as_mapping()?;
    for (key, hint) in [
        ("gates", "`gates:` was renamed to `checks:` in 0.4 — rename the top-level key."),
        ("schema_version", "this looks like a pre-0.3 config (found `schema_version:`)."),
        ("rules", "this looks like a pre-0.3 config (found `rules:`)."),
        ("trust", "this looks like a pre-0.3 config (found `trust:`)."),
    ] {
        if map.contains_key(serde_yaml::Value::String(key.into())) {
            return Some((key, hint));
        }
    }
    None
}
```
Update `parse_str` (L9-16) to use the returned hint in the error. Keep the existing legacy tests passing (they assert the error mentions the format).

- [ ] **Step 3: Write the failing test for the new marker.** In `parser.rs` tests:

```rust
#[test]
fn rejects_legacy_gates_key() {
    let err = parse_str("gates:\n  g:\n    files: \"*\"\n    run: \"true\"\n")
        .unwrap_err()
        .to_string();
    assert!(err.contains("checks"), "error should point at `checks:`: {err}");
}
```

- [ ] **Step 4: Run it, confirm it fails**, then confirm Step 2 makes it pass. `cargo test -p hector-core config::parser`.

- [ ] **Step 5: Migrate the dogfood `.hector.yml`.** Change `gates:` → `checks:`. **Keep the `case $?; exit 2` runs unchanged** (they still block under the unchanged classification; the `! grep` simplification is Phase 6). Update `mod.rs` re-exports (`Gate`→`Check`).

- [ ] **Step 6: `cargo build` will now fail across runner/engine/verdict/cli** — that's expected; Tasks 1.2-1.4 fix the consumers. Do NOT commit yet.

### Task 1.2: Rename in engine + runner

**Files:**
- Modify: `crates/hector-core/src/engine/gate.rs` (`GateEnv.event` comment: drop `edit` from the doc list only)
- Modify: `crates/hector-core/src/runner.rs:14,69,378-417,432-473` + tests (L508-680)

- [ ] **Step 1: Rename runner symbols.** `CheckOptions.gates` → `checks` (the `HashSet<String>` id filter); `HectorEngine.scope_matchers` keys are check ids (no type change); `for (id, gate) in &self.config.gates` → `&self.config.checks`; `run_one_gate` → `run_one_check`; `gate: &Gate` params → `check: &Check`; `engine.gates()` accessor → `checks()`; `gate_ids()` → `check_ids()`. Update the 9 runner tests' identifiers and any `gates:` YAML in their fixtures.

- [ ] **Step 2: Update `gate.rs` doc comment** for `GateEnv.event` (L20) to list `write`/`pre-commit` (drop `edit` and `manual`). No code change here yet (the enum/validation is Phase 5).

- [ ] **Step 3: `cargo test -p hector-core`** — engine + runner now compile; verdict/telemetry still pending (1.3).

### Task 1.3: Schema 5 — verdict + telemetry key rename

**Files:**
- Modify: `crates/hector-core/src/verdict.rs:5,34-48` + tests (L91-150)
- Modify: `crates/hector-core/src/telemetry.rs:18,22-45` + tests (L111-132)

**Interfaces:**
- Produces: `Block { check: String, step: Option<String>, file: String, message }`; `GateError { check: String, step: Option<String>, file: Option<String>, reason }`; `SCHEMA_VERSION = 5`; telemetry `PerCheckRecord { check, step: Option<String>, status, elapsed_ms, reason }`, `LogEntry::Check { ts, file, event, status, elapsed_ms, checks }`, telemetry `SCHEMA_VERSION = 4`.

- [ ] **Step 1: Write the failing schema test.** In `verdict.rs`, change `schema_version_is_4` (L150) to:

```rust
#[test]
fn schema_version_is_5() {
    assert_eq!(SCHEMA_VERSION, 5);
}
```

- [ ] **Step 2: Bump + rename in `verdict.rs`.** `const SCHEMA_VERSION: u32 = 5;`. On `Block`: rename field `gate`→`check`, add `pub step: Option<String>` (serialized; `#[serde(skip_serializing_if = "Option::is_none")]`). On `GateError`: rename `gate`→`check`, add `step: Option<String>`, change `file: String`→`file: Option<String>`. Update `Verdict::from_outcomes` (and any constructor) to pass `step: None`, `file: Some(..)` for now (steps populate `step` in Phase 3).

- [ ] **Step 3: Write the JSON-key test:**

```rust
#[test]
fn block_serializes_check_key_not_gate() {
    let b = Block { check: "rustfmt".into(), step: None, file: "a.rs".into(), message: "x".into() };
    let j = serde_json::to_string(&b).unwrap();
    assert!(j.contains("\"check\":\"rustfmt\""), "{j}");
    assert!(!j.contains("\"gate\""), "{j}");
}
```

- [ ] **Step 4: Telemetry.** `PerGateRecord` → `PerCheckRecord`, field `gate`→`check`, add `step: Option<String>`. `LogEntry::Check` gains `event: String`. Bump telemetry `SCHEMA_VERSION = 4`; change `schema_version_is_3` → `schema_version_is_4`. Update `round_trips_a_gate_check_entry` to populate `event` + the new key.

- [ ] **Step 5: Thread `event` into the log.** In `runner.rs` `append_check_log` (L473) + its call site (L457), pass `self.options.event.clone()` into `LogEntry::Check { event, .. }`.

- [ ] **Step 6:** `cargo test -p hector-core` green.

### Task 1.4: Rename in the CLI + tests

**Files:**
- Modify: `crates/hector-cli/src/cli.rs:18,37,68`
- Modify: `crates/hector-cli/src/commands/check.rs:38-101`
- Modify: `crates/hector-cli/src/commands/explain.rs:14,33`, `show_resolved_config.rs`
- Modify: `crates/hector-cli/tests/cli_e2e_gates.rs`, `cli_validate.rs`, `cli_e2e_explain.rs` (fixtures + assertions)

- [ ] **Step 1: Rename the flag.** `cli.rs:18` `--gate` → `--check`: `#[arg(long = "check")] checks: Vec<String>`. Update help text (L37, L68) `gate`→`check`. Thread the rename through `main.rs` → `check::run(.. checks ..)`.

- [ ] **Step 2: Rename in `check.rs`.** `validate_gate_filter`→`validate_check_filter`; `engine.gate_ids()`→`check_ids()`; error `"unknown gate id(s)"`→`"unknown check id(s)"`; `print_explain` uses `row.check_id`; `emit` prints `b.check`/`e.check`; `set_gate_filter`→`set_check_filter`.

- [ ] **Step 3: Rename in `explain.rs`/`show_resolved_config.rs`.** `ExplainEntry.gate`→`check`; `engine.gates()`→`checks()`; output strings `gate`→`check`.

- [ ] **Step 4: Migrate CLI test fixtures + assertions.** In `cli_e2e_gates.rs`: every inline `gates:` YAML → `checks:`; rename test fns referencing "gate" if desired (optional). In `cli_validate.rs`: `validate_accepts_valid_gates_config` asserts `"ok: N gate(s)"` → update both the assertion and the `validate.rs` output string to `"check(s)"`. In `cli_e2e_explain.rs`: update expected `gate`→`check` strings.

- [ ] **Step 5: `cargo test` (whole workspace) green; `cargo clippy --all-targets -- -D warnings`; `cargo fmt`.**

- [ ] **Step 6: Commit the whole keystone.**

```bash
git add -A -- ':!Cargo.lock'
git commit -m "refactor: rename gates->checks, bump verdict schema 4->5

Mechanical vocabulary rename across config/engine/runner/verdict/telemetry/cli
plus the dogfood .hector.yml. Behavior identical (exit 2 still blocks); only
JSON keys (gate->check) and schema numbers change. Legacy 'gates:' now rejected
with a curated 'renamed to checks:' message.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KCX6mWmrFwyUeEA2FxhPVU"
```

---

# Phase 2 — Config grammar: steps, on, name

Turn `Check` into the steps model. `run`/`steps` are mutually exclusive; `Check::effective_steps()` normalizes a `run` into a one-element step list. `on:` defaults to `[write]`. The runner starts iterating steps (fail-fast) under the *current* exit-2 classification — the flip is Phase 3.

### Task 2.1: `Step`, `Lifecycle`, and the new `Check` shape

**Files:**
- Modify: `crates/hector-core/src/config/types.rs`
- Modify: `crates/hector-core/src/config/mod.rs`

**Interfaces:**
- Produces:
  - `enum Lifecycle { Write, PreCommit }` (`#[serde(rename_all = "kebab-case")]` → `write`, `pre-commit`)
  - `struct Step { name: Option<String>, run: String }`
  - `struct Check { files: Vec<String>, run: Option<String>, steps: Option<Vec<Step>>, on: Vec<Lifecycle>, name: Option<String> }`
  - `impl Check { pub fn effective_steps(&self) -> Vec<Step> }` — `run` → `vec![Step { name: None, run }]`, else `steps.clone()`. Relies on parser validation guaranteeing exactly one is set.

- [ ] **Step 1: Write the failing tests** in `types.rs`:

```rust
#[test]
fn on_defaults_to_write() {
    let cfg: Config = serde_yaml::from_str(
        "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n").unwrap();
    assert_eq!(cfg.checks["g"].on, vec![Lifecycle::Write]);
}

#[test]
fn lifecycle_parses_kebab_pre_commit() {
    let cfg: Config = serde_yaml::from_str(
        "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n    on: [write, pre-commit]\n").unwrap();
    assert_eq!(cfg.checks["g"].on, vec![Lifecycle::Write, Lifecycle::PreCommit]);
}

#[test]
fn run_normalizes_to_one_step() {
    let cfg: Config = serde_yaml::from_str(
        "checks:\n  g:\n    files: \"*\"\n    run: \"rustfmt\"\n").unwrap();
    let steps = cfg.checks["g"].effective_steps();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].run, "rustfmt");
}

#[test]
fn steps_list_parses_with_names() {
    let cfg: Config = serde_yaml::from_str(
        "checks:\n  g:\n    files: \"*\"\n    steps:\n      - name: a\n        run: \"true\"\n      - run: \"false\"\n").unwrap();
    let steps = cfg.checks["g"].effective_steps();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].name.as_deref(), Some("a"));
    assert_eq!(steps[1].name, None);
}
```

- [ ] **Step 2: Run them, confirm they fail** (`Lifecycle`/`Step`/`effective_steps` undefined). `cargo test -p hector-core config::types`.

- [ ] **Step 3: Implement the types.** Add `Lifecycle`, `Step`, the `default_on()` fn, and the new `Check` fields with serde defaults:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Lifecycle { Write, PreCommit }

fn default_on() -> Vec<Lifecycle> { vec![Lifecycle::Write] }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub run: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    #[serde(deserialize_with = "files_one_or_many")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<Step>>,
    #[serde(default = "default_on")]
    pub on: Vec<Lifecycle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Check {
    /// The check's work as a step list. `run` is one-step sugar.
    /// Parser validation guarantees exactly one of run/steps is set.
    pub fn effective_steps(&self) -> Vec<Step> {
        if let Some(run) = &self.run {
            vec![Step { name: None, run: run.clone() }]
        } else {
            self.steps.clone().unwrap_or_default()
        }
    }
}
```

- [ ] **Step 4: Run the tests, confirm pass.** Update the Task 1.1 tests that constructed `Check { files, run }` directly (now `run` is `Option`). Re-export `Check, Step, Lifecycle` from `mod.rs`.

- [ ] **Step 5: `cargo build -p hector-core` will break** at `check.run` (String) usages in runner — fixed in Task 2.3. Commit after 2.2/2.3 land together.

### Task 2.2: run-xor-steps + per-step validation

**Files:**
- Modify: `crates/hector-core/src/config/parser.rs:9-29` + tests

- [ ] **Step 1: Write the failing tests** in `parser.rs`:

```rust
#[test]
fn rejects_check_with_both_run_and_steps() {
    let err = parse_str("checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n    steps:\n      - run: \"true\"\n").unwrap_err().to_string();
    assert!(err.contains("`g`") && err.contains("run") && err.contains("steps"), "{err}");
}

#[test]
fn rejects_check_with_neither_run_nor_steps() {
    let err = parse_str("checks:\n  g:\n    files: \"*\"\n").unwrap_err().to_string();
    assert!(err.contains("`g`"), "{err}");
}

#[test]
fn rejects_step_with_comment_only_run() {
    let err = parse_str("checks:\n  g:\n    files: \"*\"\n    steps:\n      - run: \"# nope\"\n").unwrap_err().to_string();
    assert!(err.contains("`g`"), "{err}");
}
```

- [ ] **Step 2: Confirm they fail.** `cargo test -p hector-core config::parser`.

- [ ] **Step 3: Implement validation** in `parse_str`, replacing the old single-`run` loop (L18-27):

```rust
for (id, check) in &cfg.checks {
    match (&check.run, &check.steps) {
        (Some(_), Some(_)) => return Err(anyhow!(
            "check `{id}` has both `run` and `steps` — use one (a single `run` is one-step sugar)")),
        (None, None) => return Err(anyhow!(
            "check `{id}` has neither `run` nor `steps` — a check must do something")),
        (Some(run), None) => guard_run(id, None, run)?,
        (None, Some(steps)) => {
            for (i, step) in steps.iter().enumerate() {
                guard_run(id, Some(i), &step.run)?;
            }
        }
    }
}
```
Add a `guard_run` helper wrapping the existing `run_has_executable_content` check (preserving the block-scalar/comment-collapse error message; include the step index when `Some`).

- [ ] **Step 4: Run all parser tests, confirm pass** (including the pre-existing `rejects_run_that_is_only_a_comment` / `rejects_empty_run`, now routed through `guard_run`).

### Task 2.3: Runner iterates steps (fail-fast, current classification)

**Files:**
- Modify: `crates/hector-core/src/runner.rs:393-417` + tests

**Interfaces:**
- Consumes: `Check::effective_steps()`.
- Produces: `run_one_check` runs each step in order, returns the first non-Pass outcome (fail-fast); on Block from a *named* step, the outcome carries the step name.

- [ ] **Step 1: Write the failing test** in `runner.rs` tests:

```rust
#[test]
fn steps_fail_fast_on_first_blocking_step() {
    // step 1 passes (exit 0), step 2 blocks (exit 2 under current classification),
    // step 3 must NOT run. Use a check whose step 3 would touch a sentinel file.
    let dir = tempdir().unwrap();
    write_config(&dir, "checks:\n  g:\n    files: \"*\"\n    steps:\n      - run: \"true\"\n      - name: blocker\n        run: \"echo nope; exit 2\"\n      - run: \"touch ran3.txt\"\n");
    let engine = load(&dir);
    let v = engine.check(file_input(&dir, "x.txt", "body")).unwrap();
    assert_eq!(v.status, Status::Block);
    assert_eq!(v.blocks[0].step.as_deref(), Some("blocker"));
    assert!(!dir.path().join("ran3.txt").exists(), "step 3 ran after a block");
}
```
(Reuse the existing test harness helpers in `runner.rs` tests; mirror their `tempdir`/`write_config`/`load` patterns.)

- [ ] **Step 2: Confirm it fails** (currently single-`run`, no `step` on Block).

- [ ] **Step 3: Implement the steps loop.** In `run_one_check`, replace the single `run_gate(&check.run, &env, ..)` call with:

```rust
for step in check.effective_steps() {
    match run_gate(&step.run, &env, Some(content.as_bytes()), self.timeout) {
        GateOutcome::Pass => continue,
        GateOutcome::Block { message } => {
            return CheckStatus::Block { step: step.name.clone(), message };
        }
        GateOutcome::Internal(reason) => {
            return CheckStatus::Error { step: step.name.clone(), reason };
        }
    }
}
CheckStatus::Pass
```
Add `step: Option<String>` to the `CheckStatus::Block`/`Error` variants and thread it into `Block.step`/`GateError.step` in `absorb`/`from_outcomes`.

- [ ] **Step 4: Run tests, confirm pass.** `cargo test -p hector-core`.

- [ ] **Step 5: `cargo clippy --all-targets -- -D warnings`; `cargo fmt`; commit Phase 2.**

```bash
git add -A -- ':!Cargo.lock'
git commit -m "feat(config): steps/on/name grammar; runner iterates steps fail-fast

A check is now files + (run xor steps) + on (default [write]) + name. run is
one-step sugar via Check::effective_steps(). Runner runs steps in order,
fail-fast; the blocking step's name rides into the verdict. Classification
unchanged (exit 2 blocks) — the flip is Phase 3.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KCX6mWmrFwyUeEA2FxhPVU"
```

---

# Phase 3 — Engine: nonzero blocks + `$HECTOR_FILES`

Flip `classify()` so `1`–`125` blocks; preserve the broken-gate fail-open tier. Add `$HECTOR_FILES` to the ABI.

### Task 3.1: Classification flip

**Files:**
- Modify: `crates/hector-core/src/engine/gate.rs:124-146` + tests (L166-275)

- [ ] **Step 1: Invert the 3 affected unit tests.** Rewrite `exit_one_is_pass` → `exit_one_is_block`:

```rust
#[test]
fn exit_one_is_block() {
    let out = run("exit 1", env(), None, secs(5));
    assert!(matches!(out, GateOutcome::Block { .. }), "exit 1 must block (nonzero = block)");
}
```
Keep `exit_two_is_block_with_message` (2 ∈ 1–125, still blocks — assertion unchanged). Add `exit_125_is_block` and keep `exit_zero_is_pass`. Keep `command_not_found_is_internal` (127), `timeout_is_internal`, `high_normal_exit_is_internal_with_exit_code_label` (≥128) unchanged.

- [ ] **Step 2: Confirm the suite fails** on `exit_one_is_block`. `cargo test -p hector-core engine::gate`.

- [ ] **Step 3: Flip `classify()`:**

```rust
fn classify(status: ExitStatus, stdout: &str, stderr: &str) -> GateOutcome {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return GateOutcome::Internal(InternalReason::Signal(sig));
        }
    }
    match status.code() {
        Some(0) => GateOutcome::Pass,
        Some(126) => GateOutcome::Internal(InternalReason::NotExecutable),
        Some(127) => GateOutcome::Internal(InternalReason::NotFound),
        Some(c) if c >= 128 => GateOutcome::Internal(InternalReason::HighExit(c)),
        Some(c) if (1..=125).contains(&c) => {
            let message = [stdout.trim(), stderr.trim()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            GateOutcome::Block { message }
        }
        // None == terminated without code on non-unix; treat as internal.
        _ => GateOutcome::Internal(InternalReason::HighExit(-1)),
    }
}
```

- [ ] **Step 4: Run, confirm pass.** `cargo test -p hector-core`.

### Task 3.2: Add `$HECTOR_FILES` to the ABI

**Files:**
- Modify: `crates/hector-core/src/engine/gate.rs:14-21,61-91` + tests
- Modify: `crates/hector-core/src/runner.rs:404-410` (build the new `GateEnv`)

**Interfaces:**
- Produces: `GateEnv { file: Option<&Path>, files: &'a [PathBuf], root: &Path, event: &str }`. `run_gate` sets `$HECTOR_FILE` only when `file.is_some()`, and always sets `$HECTOR_FILES` = the `files` joined by `\n` (absolute paths).

- [ ] **Step 1: Write the failing test** in `gate.rs`:

```rust
#[test]
fn hector_files_is_exported_newline_joined() {
    // gate greps $HECTOR_FILES for two paths; blocks (exit 1) if both present.
    let out = run(
        "grep -q a.rs <<<\"$HECTOR_FILES\" && grep -q b.rs <<<\"$HECTOR_FILES\" && exit 1 || exit 0",
        env_with_files(&["/p/a.rs", "/p/b.rs"]),
        None, secs(5));
    assert!(matches!(out, GateOutcome::Block { .. }), "both files must be in $HECTOR_FILES");
}
```
Add an `env_with_files` test helper paralleling the existing `env()` helper.

- [ ] **Step 2: Confirm it fails** (`$HECTOR_FILES` unset → grep finds nothing → exit 0 → Pass).

- [ ] **Step 3: Change `GateEnv`** to `file: Option<&Path>` + `files: &[PathBuf]`. In `run_gate` env setup (L61-69): set `HECTOR_FILE` only `if let Some(f) = env.file`; always `.env("HECTOR_FILES", env.files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"))`.

- [ ] **Step 4: Update the runner caller** (`run_one_check`, ~L404): build `GateEnv { file: Some(&abs), files: std::slice::from_ref(&abs), root, event }` (per-file: the set is the single file). Update the existing `hector_file_env_is_exported` test if its `GateEnv` literal changed.

- [ ] **Step 5: Run tests, confirm pass; clippy; fmt; commit Phase 3.**

```bash
git add -A -- ':!Cargo.lock'
git commit -m "feat(engine): nonzero (1-125) blocks; add \$HECTOR_FILES to the ABI

classify() now blocks on any ordinary failure (1-125), keeping the broken-gate
fail-open tier (126/127/>=128/signal/timeout -> internal). GateEnv exports
\$HECTOR_FILES (newline-joined) in addition to \$HECTOR_FILE.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KCX6mWmrFwyUeEA2FxhPVU"
```

---

# Phase 4 — Runner: lifecycle dispatch + `on:` filter

`write` runs per-file (current path, now also setting `$HECTOR_FILES` = the one file). `pre-commit` runs each check **once** over the changed set with `$HECTOR_FILES`, `$HECTOR_FILE` unset, empty stdin. The `on:` filter skips checks whose `on` excludes the current event; the event is always `write` or `pre-commit`, so the filter always applies.

### Task 4.1: The `on:` filter

**Files:**
- Modify: `crates/hector-core/src/runner.rs:378-391` (`skip_reason`) + `CheckOptions` (L14) + tests

**Interfaces:**
- Consumes: `Check.on: Vec<Lifecycle>`, `CheckOptions.event: String`.
- Produces: a check is skipped when its `on` does not contain the current lifecycle. The event is always `write` or `pre-commit` and maps directly to a `Lifecycle`; the filter always applies (no bypass).

- [ ] **Step 1: Write failing tests** in `runner.rs`:

```rust
#[test]
fn on_filter_skips_write_only_check_at_pre_commit() {
    let dir = tempdir().unwrap();
    write_config(&dir, "checks:\n  g:\n    files: \"*\"\n    run: \"exit 2\"\n"); // on defaults [write]
    let engine = load_with_event(&dir, "pre-commit");
    let v = engine.check(file_input(&dir, "x.txt", "b")).unwrap();
    assert_eq!(v.status, Status::Pass, "write-only check must not run at pre-commit");
}

#[test]
fn on_filter_runs_check_subscribed_to_event() {
    let dir = tempdir().unwrap();
    write_config(&dir, "checks:\n  g:\n    files: \"*\"\n    on: [pre-commit]\n    run: \"exit 2\"\n");
    let engine = load_with_event(&dir, "pre-commit");
    let v = engine.check(file_input(&dir, "x.txt", "b")).unwrap();
    assert_eq!(v.status, Status::Block, "a pre-commit check must run at pre-commit");
}
```
Add a `load_with_event` helper (sets `CheckOptions.event`).

- [ ] **Step 2: Confirm they fail.**

- [ ] **Step 3: Implement.** Add a total helper `event_lifecycle(event: &str) -> Lifecycle` (`"pre-commit"`→`PreCommit`, everything else→`Write`; the CLI value_parser guarantees only `write`/`pre-commit` reach here). In `skip_reason`, after the existing filter/scope/disable checks, add: if `!check.on.contains(&event_lifecycle(&self.options.event))`, return `Some("event")`.

- [ ] **Step 4: Run tests, confirm pass.**

### Task 4.2: Pre-commit run-once dispatch

**Files:**
- Modify: `crates/hector-core/src/runner.rs` (add a set-input path) + tests

**Interfaces:**
- Produces: `HectorEngine::check_set(files: &[PathBuf]) -> Result<Verdict>` — for each check whose scope matches ≥1 file in the set and whose `on` admits the event, run the check's steps **once** with `GateEnv { file: None, files: &matched, .. }`, empty stdin. Folds into the same `Verdict`.

- [ ] **Step 1: Write the failing test:**

```rust
#[test]
fn pre_commit_runs_check_once_over_the_set() {
    let dir = tempdir().unwrap();
    // check counts how many times it runs by appending to a file; asserts ONE run.
    write_config(&dir, "checks:\n  g:\n    files: \"*.rs\"\n    on: [pre-commit]\n    run: \"printf x >> $HECTOR_ROOT/runs.txt; test $(grep -c x $HECTOR_ROOT/runs.txt 2>/dev/null || echo 0) -le 1\"\n");
    touch(&dir, "a.rs"); touch(&dir, "b.rs");
    let engine = load_with_event(&dir, "pre-commit");
    let v = engine.check_set(&[abs(&dir,"a.rs"), abs(&dir,"b.rs")]).unwrap();
    let runs = std::fs::read_to_string(dir.path().join("runs.txt")).unwrap_or_default();
    assert_eq!(runs.len(), 1, "check must run exactly once over the set, got {runs:?}");
    assert_eq!(v.status, Status::Pass);
}
```

- [ ] **Step 2: Confirm it fails** (`check_set` undefined).

- [ ] **Step 3: Implement `check_set`.** Mirror `check_inner`, but the inner loop runs each check once: compute `matched: Vec<PathBuf>` = set members whose path matches the check's `ScopeMatcher`; skip the check if `matched` is empty or `skip_reason` (event/filter) rejects it; build `GateEnv { file: None, files: &matched, root, event }`; run `effective_steps()` fail-fast with `content` = empty (`Some(b"")` or `None` per the stdin contract — use `None` so stdin closes). Telemetry: one record per check, `file: None`.

- [ ] **Step 4: Run tests, confirm pass; clippy; fmt; commit Phase 4.**

```bash
git add -A -- ':!Cargo.lock'
git commit -m "feat(runner): on: filter + pre-commit run-once dispatch

Checks are skipped when their on: excludes the event (write or pre-commit).
check_set() runs each matching check once over the changed set with
\$HECTOR_FILES and no stdin — the pre-commit / lefthook-parity path.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KCX6mWmrFwyUeEA2FxhPVU"
```

---

# Phase 5 — CLI: `--event` lifecycle, `--diff` pre-commit

### Task 5.1: Drop `edit`; wire `--diff --event pre-commit` to `check_set`

**Files:**
- Modify: `crates/hector-cli/src/cli.rs` (`--event` value parser)
- Modify: `crates/hector-cli/src/commands/check.rs` (`run_diff`)
- Modify: `crates/hector-cli/tests/cli_event_validation.rs`, `cli_e2e_gates.rs`

- [ ] **Step 1: Update the event-validation tests.** In `cli_event_validation.rs`: `event_edit_is_accepted` → `event_write_is_accepted` (`--event write` exits 0). `event_bogus_is_rejected_and_lists_valid_values` now expects the list `[write, pre-commit]` (no `edit`, no `manual`).

- [ ] **Step 2: Confirm they fail**, then set the `--event` `value_parser` to `["write", "pre-commit"]` with `default_value = "write"` in `cli.rs`. Change `CheckOptions`'s default `event` from `"manual"` to `"write"` (runner.rs `CheckOptions`, L14 area), and drop any `"edit"`/`"manual"` handling anywhere it appears.

- [ ] **Step 3: Write the failing e2e** in `cli_e2e_gates.rs`:

```rust
#[test]
fn diff_pre_commit_runs_check_once_over_set() {
    // two .rs files in the diff, a pre-commit check that blocks if $HECTOR_FILES has 2 lines
    // ... build a temp repo + unified diff touching a.rs and b.rs ...
    cmd().args(["check","--diff","d.patch","--event","pre-commit","--config",".hector.yml"])
        .assert().code(2);
}
```

- [ ] **Step 4: Confirm it fails**, then implement in `run_diff`: when `event == "pre-commit"`, collect the changed (non-`Deleted`) paths into a `Vec<PathBuf>` and call `engine.check_set(&paths)` once, instead of the current per-file loop. For other events keep the per-file loop. Emit/`exit_code` unchanged.

- [ ] **Step 5: Run the whole workspace suite, confirm pass; clippy; fmt; commit Phase 5.**

```bash
git add -A -- ':!Cargo.lock'
git commit -m "feat(cli): drop edit event; --diff --event pre-commit runs checks once

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KCX6mWmrFwyUeEA2FxhPVU"
```

---

# Phase 6 — Dogfood DX + docs repositioning

Non-behavioral: simplify the dogfood config to the new idiom and reposition the prose. No `cargo test` semantics change, but `scripts/ci-dogfood.sh` must still go red on a real violation.

### Task 6.1: Simplify `.hector.yml` to the nonzero-blocks idiom

**Files:**
- Modify: `.hector.yml`
- Verify: `scripts/ci-dogfood.sh`

- [ ] **Step 1:** Replace each gate's `grep ...; case $? in 0) exit 2;; 1) exit 0;; *) exit $?;; esac` with the stdin forbid idiom, e.g.:

```yaml
checks:
  no-todo-in-src:
    files: crates/*/src/**/*.rs
    run: '! grep -nE "todo!\\(" "$HECTOR_FILE"'
  no-unimplemented-in-src:
    files: crates/*/src/**/*.rs
    run: '! grep -nE "unimplemented!\\(" "$HECTOR_FILE"'
  no-dbg-in-src:
    files: crates/*/src/**/*.rs
    run: '! grep -nE "dbg!\\(" "$HECTOR_FILE"'
```

- [ ] **Step 2: Re-bless trust** (the gates-dir/config hash changed): run `cargo run -q -- trust` (or `./target/release/hector trust`) so `~/.config/hector/trust.json` matches; confirm `hector check` on a dirty file still exits 2.

- [ ] **Step 3: Run `bash scripts/ci-dogfood.sh`** (or its commands) against a file containing `todo!()` and confirm it blocks; against a clean file and confirm it passes.

- [ ] **Step 4: Commit.**

```bash
git add .hector.yml
git commit -m "chore(dogfood): adopt nonzero-blocks idiom in hector's own checks

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KCX6mWmrFwyUeEA2FxhPVU"
```

### Task 6.2: Reposition README + the `hector-config` skill

**Files:**
- Modify: `README.md`, the `hector-config` skill (`SKILL.md` + `hector schema` output if it embeds the grammar), `CHANGELOG.md`

- [ ] **Step 1:** Update the pitch to "a local CI for agents"; replace `gates:`/`gate`/exit-2 examples with `checks:`/`on:`/`steps:`/nonzero-blocks; document the `write`/`pre-commit` lifecycles, `$HECTOR_FILE`/`$HECTOR_FILES`/stdin ABI, and the lefthook-parity story (spec §8). Note `hector-disable: <check-id>` vocabulary.

- [ ] **Step 2:** If `hector schema` (the skill's machine surface) embeds the config grammar, regenerate/update it for `checks`/`run`/`steps`/`on`/`name`.

- [ ] **Step 3: `cargo test`** (skill/schema snapshot tests, if any — `cargo insta review` on intentional changes); commit.

```bash
git add -A -- ':!Cargo.lock'
git commit -m "docs: reposition as 'local CI for agents'; checks vocabulary + lifecycles

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KCX6mWmrFwyUeEA2FxhPVU"
```

---

## Final verification

- [ ] `cargo test` (whole workspace) green.
- [ ] `cargo clippy --all-targets -- -D warnings` clean.
- [ ] `cargo fmt --check` clean.
- [ ] `bash scripts/ci-coverage.sh` — every touched file ≥80% region (CI; not runnable locally).
- [ ] `cargo build --release` then sanity-check the three flows by hand: a `write` per-file block, a `pre-commit --diff` run-once block, and a clean pass. Then `cargo clean -p hector-cli` to drop the release artifact.
- [ ] Spot-check a surviving-mutant pass on the two highest-risk files: `cargo mutants --file crates/hector-core/src/engine/gate.rs` and `--file crates/hector-core/src/runner.rs` (local, ad-hoc). Treat survivors in `classify`/`check_set` as coverage gaps.

## Spec coverage check

- §2 config language → Phase 1 (rename) + Phase 2 (steps/on/name). ✓
- §3 example file (`! grep`, depcruise, prettier) → idioms exercised in Phase 2/4 tests + Phase 6 dogfood. ✓
- §4 nonzero blocks + broken-gate tier → Task 3.1. ✓
- §5 steps fail-fast + lifecycle shapes → Task 2.3 (fail-fast) + Task 4.2 (run-once). ✓
- §6 ABI (`$HECTOR_FILES`, lifecycle channels) → Task 3.2 + Task 4.2. ✓
- §7 schema 5 / `check`/`step` keys / telemetry → Task 1.3. ✓
- §8 lefthook parity → Task 4.2 + Task 6.2 docs. ✓
- §9 code impact → all phases. ✓
- §12 resolved decisions → fail-fast (2.3), on default [write] (2.1), pre-commit run-once (4.2), `$HECTOR_FILES` both lifecycles (3.2/4.2), no `uses:` (not added), rename (1.x). ✓
- **`--event`** — defaults to `write`; events are `write`/`pre-commit` only (no `manual`/`edit`); the `on:` filter always applies. ✓ (Task 4.1, 5.1)
