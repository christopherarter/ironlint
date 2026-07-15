# Doctor Module Split Completion Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development task-by-task, with an independent review after each task.

**Goal:** Finish the doctor command structural split by keeping `mod.rs` a small report facade and placing focused tests in matching test modules without changing doctor output.

**Architecture:** `doctor/config.rs` owns configuration and script diagnostics; `doctor/adapters.rs` owns harness, hook, and dependency diagnostics; `doctor/mod.rs` owns report assembly and CLI output. Test files mirror these responsibilities and import sibling helpers directly through `pub(super)` visibility.

**Tech Stack:** Rust, `anyhow`, existing IronLint adapter diagnostics, Cargo test/clippy, cargo-llvm-cov.

## Global Constraints

- Preserve human and JSON doctor report contents, order, status, remediation, and exit behavior.
- No new doctor feature or probe; this is organization only.
- Keep production/test helpers at `pub(super)` only when the sibling test subtree needs them; do not widen public API.
- Rust source files must remain at or above 80% region coverage and complexity must remain at or below 15.

---

### Task 1: Reduce the doctor facade to report assembly

**Files:**
- Modify: `crates/ironlint-cli/src/commands/doctor/mod.rs`

**Interfaces:**
- Consumes: `config::config_section` and `adapters::adapter_section`.
- Produces: the unchanged `pub fn run(dir: &Path, json: bool) -> Result<i32>` plus report types and rendering only.

- [ ] Write an assertion-preserving compilation check before moving imports.

Run: `cargo test -p ironlint-cli commands::doctor`

- [ ] Remove temporary test-only imports and retain only imports needed by production report assembly.

- [ ] Run focused doctor tests, fmt, and clippy.

```bash
cargo fmt --check
cargo clippy -p ironlint-cli --all-targets -- -D warnings
cargo test -p ironlint-cli commands::doctor
```

- [ ] Commit the facade-only change.

```bash
git add crates/ironlint-cli/src/commands/doctor/mod.rs
git commit -m "refactor-doctor-reduce-facade"
```

### Task 2: Split doctor tests by responsibility

**Files:**
- Modify: `crates/ironlint-cli/src/commands/doctor/mod.rs`
- Create: `crates/ironlint-cli/src/commands/doctor/tests/mod.rs`
- Create: `crates/ironlint-cli/src/commands/doctor/tests/config.rs`
- Create: `crates/ironlint-cli/src/commands/doctor/tests/adapters.rs`
- Create: `crates/ironlint-cli/src/commands/doctor/tests/report.rs`

**Interfaces:**
- Consumes: `doctor::config` and `doctor::adapters` helpers through `pub(super)` visibility.
- Produces: the same test set, discoverable via `#[cfg(test)] mod tests;`.

- [ ] Move configuration/script tests into `tests/config.rs`, adapter/hook/dependency tests into `tests/adapters.rs`, and report rendering tests into `tests/report.rs` without assertion changes.

- [ ] Keep shared test fixtures in `tests/mod.rs`; leaf modules import all direct production and external dependencies.

- [ ] Make `mod.rs` contain only `#[cfg(test)] mod tests;` test wiring; no root test imports or inline test block.

- [ ] Run focused doctor tests, full CLI tests, clippy, and the exact coverage gate.

```bash
cargo test -p ironlint-cli commands::doctor
cargo test -p ironlint-cli
cargo clippy --all-targets -- -D warnings
bash scripts/ci-coverage.sh
```

- [ ] Commit the relocated tests.

```bash
git add crates/ironlint-cli/src/commands/doctor
git commit -m "test-doctor-split-modules"
```

### Task 3: Review doctor and the whole branch

- [ ] Dispatch an independent adversarial review of the doctor split, requiring concrete report-content or test-discovery scenarios.
- [ ] Dispatch one final whole-branch reviewer from merge base `bbb6ed7`, covering trust, watch, doctor, PTY test, visibility, behavior preservation, and coverage-policy compliance.
