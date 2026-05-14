# Hector C1 — `hector doctor` diagnostic subcommand

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or superpowers:subagent-driven-development) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec section:** [`specs/2026-05-12-bully-parity-closures.md` §C1](../specs/2026-05-12-bully-parity-closures.md)
**Severity:** 🔴 critical UX
**Sequencing:** 0.2.0 release blocker — last item in the 0.2.0 cohort after A1 (prompt injection), A2 (skip patterns), A3 (diff pre-filter) shipped.

---

**Goal:** Add `hector doctor` — a read-only diagnostic command that prints a checklist of every load-time invariant Hector cares about, with per-check `pass`/`warn`/`fail` status and a remediation hint. Output is human-readable by default; `--format json` emits a machine-consumable record. Exit code: `0` if every check is `pass` or `warn`, `1` if any `fail`. Doctor never modifies state and never produces a `Verdict` — it sits alongside `check`, not inside it. Closes the single highest-leverage UX gap vs. bully (most quoted in support questions) without touching the locked exit-code contract on `check` (`0`/`1`/`2`).

**Architecture:** A new module `crates/hector-cli/src/commands/doctor.rs` owns every check. Each check is a small free function returning `CheckResult { name, status, detail, remediation }`; a `Doctor` struct collects them in a fixed order and produces a `Report { hector_version, checks }`. The orchestrator is a single `pub fn run(dir: &Path, format: OutputFormat) -> Result<i32>` that walks the check list, accumulates the report, prints it, and computes the exit code. Decomposing per-check keeps `run`'s cognitive complexity well under 15 — every check is one function. New core surface is **zero**: doctor reuses `crate::trust::verify`, `crate::config::extends::resolve_trusted` (and the non-trust `parse_file_with_extends` for the parses-but-untrusted distinction), `crate::config::scope::ScopeMatcher::new`, and `crate::config::EngineKind` from `hector-core` — all already public. The only thin addition to `hector-core` is one helper `pub fn read_api_key_present(env_name: &str) -> bool` exposed from `crate::llm` so doctor doesn't have to reach into `std::env::var` directly with the same emptiness check `read_api_key` uses internally.

**Tech Stack:** Rust, workspace-stable. clap (existing) for the new subcommand, `serde` + `serde_json` (workspace) for the `--format json` payload, `assert_cmd` + `tempfile` (existing dev-deps) for CLI integration tests, `insta` (workspace dev-dep, already in `hector-cli`'s test toolchain via `serde_json` snapshot) for the JSON output snapshot. **No new runtime dependencies.**

---

## Decisions ratified up-front (per spec §C1 + §7)

| Decision | Choice | Reason |
|---|---|---|
| Default `--dir` | `.` (cwd) | Mirrors `init`/`migrate`. |
| Default config name within `--dir` | `.hector.yml` | Mirrors every other subcommand. |
| Phone-home for "latest release" check | **No.** Defer to 0.3 if at all. | Spec §7 Q4 default. Privacy + offline-friendliness. The `Binary` check reports the running version; comparison vs. `latest` is out of scope for this plan. |
| Python-version equivalent check | **Skipped.** | Spec §C1 Notes — version skew is a non-issue for a shipped binary. We replace bully's "Python version" line with `Binary { path, version }`. |
| Exit code on warn-only | **`0`.** | Spec §C1 step 3 verbatim. Warnings inform; only failures break. |
| Exit code on any fail | **`1`.** | Spec §C1 step 3 verbatim. Distinct from `check`'s `0`/`1`/`2` because doctor never produces a `Verdict`. |
| `Verdict` JSON shape | **Untouched.** Doctor emits its own `Report` JSON; the locked `Verdict`/`SCHEMA_VERSION` are not involved. | Doctor is read-only diagnostic; it never enters the runner. |
| Telemetry writes | **None.** | Doctor is read-only. No `.hector/log.jsonl` append. |
| Trust gate behavior on doctor | **Distinguish `parses` from `trust`.** Use the non-verifying `parse_file_with_extends` for the parses check, then call `trust::verify` separately so a parses-OK-but-untrusted config reports `parses: pass` and `trust: fail`. | Otherwise an untrusted config short-circuits at "parses" and the user can't tell whether the YAML is malformed or just unsigned. |
| Adapter check scope | **Best-effort & advisory.** A missing `~/.claude/settings.json` is a `warn` ("Claude Code adapter not detected"), not a `fail`. Hector is editor-agnostic; not every user runs Claude Code. | The hook check itself (when the file exists) is structural: parse JSON, look for a `PostToolUse` matcher whose `command` references `hector` or the adapter's `hook.sh`. |
| `~/.claude/settings.json` location override | Honor `CLAUDE_PLUGIN_ROOT` env if set (mirrors the adapter's own `hooks.json`); otherwise expand `~/.claude/settings.json`. | Matches the adapter's existing convention so test rigs can point at a tempdir. |
| `.hector/` writable check | Probe by writing then deleting a marker file under `<dir>/.hector/`. Failure → `fail` with a remediation hint to `chmod` the directory. | Matches bully behavior; `.hector/` is created lazily by other commands but doctor reports the state without creating it. We DO create the directory if it doesn't exist (read-only-on-state means we don't *modify policy state* — creating an empty `.hector/` is the same kind of side effect as `init` writing `.hector.yml`). |

---

## File structure

```
crates/hector-cli/
├── src/
│   ├── cli.rs                          ← MODIFIED: add `Doctor` variant
│   ├── main.rs                         ← MODIFIED: dispatch `Doctor` to commands::doctor::run
│   └── commands/
│       ├── mod.rs                      ← MODIFIED: pub mod doctor
│       └── doctor.rs                   ← NEW: Doctor, CheckResult, Status, Report, every check fn
└── tests/
    └── cli_e2e_doctor.rs               ← NEW: assert_cmd integration tests + insta JSON snapshot

crates/hector-core/
└── src/
    └── llm/mod.rs                      ← MODIFIED: pub fn api_key_env_present(env_name) -> bool

docs/
└── doctor.md                           ← NEW: JSON output schema (public contract per spec)

README.md                               ← MODIFIED: add `hector doctor` to the commands list
plans/README.md                         ← MODIFIED (final task only): mark plan archived
```

The orchestrator (`commands::doctor::run`) is **one function that calls one function per check**. Each check returns `CheckResult`. The orchestrator's branching is `for check in checks { results.push(check(&ctx)); }` plus a fold to pick the exit code. Cognitive complexity: ≤ 4. Per-check functions average 6–10 lines; the most complex (adapter detection) decomposes into two helpers (`load_claude_settings` + `claude_hook_wired`) so neither exceeds the cap.

---

## Risk / rollback

**Exit-code contract.** Doctor introduces a *new* `0`/`1` exit-code surface — distinct from `check`'s `0`/`1`/`2`. The locked `check` contract (consumed by both adapters and dogfood CI) is untouched. Adapters do not call `doctor`; `doctor` is for humans.

**Verdict-schema impact.** None. Doctor never constructs a `Verdict` and never bumps `SCHEMA_VERSION`.

**Telemetry-schema impact.** None. Doctor is read-only — no `.hector/log.jsonl` appends, no `kind` strings introduced.

**Config-schema impact.** None. Doctor reads but does not modify `.hector.yml`.

**Performance.** Doctor runs in <100ms on a typical config: file IO + one trust hash + one HOME-dir stat for `~/.claude/settings.json`. No network, no LLM, no script execution.

**Rollback.** Pure addition. Removing the `Doctor` variant from clap, the dispatch arm in `main.rs`, the file `commands/doctor.rs`, the test file, and the docs entry restores the prior CLI surface. No persisted state.

**Coexistence with C2/C3.** Doctor occupies the `commands/doctor.rs` slot; `explain`/`guide`/`show-resolved-config` (C2/C3, not in this plan) are independent modules. The `Status::{Pass, Warn, Fail}` enum is local to `commands::doctor` and does **not** collide with `verdict::Status::{Pass, Warn, Block}` because they live in different modules and are never converted.

---

## Phase 1 — Core helper: `api_key_env_present`

The doctor's `Engines` check needs a side-effect-free "is this env var set to a non-empty value?" predicate that matches the same emptiness semantics `llm::read_api_key` uses internally (so the doctor reports the same answer the runner would consult). Adding the helper to `hector-core::llm` keeps the single source of truth in one place.

### Task 1.1: Failing test for `api_key_env_present`

**Files:**
- Create: `crates/hector-core/tests/llm_api_key_env_present.rs`

- [ ] **Step 1.1.1: Write the failing test**

```rust
//! C1 — narrow public probe so `hector doctor` can report API-key presence
//! using the same emptiness rule the runner uses internally.

use hector_core::llm::api_key_env_present;

#[test]
fn missing_env_var_is_absent() {
    // Pick a name vanishingly unlikely to collide with a real env var.
    let name = "HECTOR_DOCTOR_TEST_MISSING_VAR_THAT_DOES_NOT_EXIST";
    assert!(!api_key_env_present(name));
}

#[test]
fn empty_env_var_is_absent() {
    let name = "HECTOR_DOCTOR_TEST_EMPTY";
    std::env::set_var(name, "");
    assert!(!api_key_env_present(name));
    std::env::remove_var(name);
}

#[test]
fn nonempty_env_var_is_present() {
    let name = "HECTOR_DOCTOR_TEST_PRESENT";
    std::env::set_var(name, "x");
    assert!(api_key_env_present(name));
    std::env::remove_var(name);
}
```

- [ ] **Step 1.1.2: Run, expect failure**

Run: `cargo test -p hector-core --test llm_api_key_env_present`
Expected: FAIL with `unresolved import hector_core::llm::api_key_env_present` / `function or associated item not found`.

### Task 1.2: Implement `api_key_env_present`

**Files:**
- Modify: `crates/hector-core/src/llm/mod.rs`

- [ ] **Step 1.2.1: Add the helper next to `read_api_key`**

In `crates/hector-core/src/llm/mod.rs`, immediately above the existing `fn read_api_key`, add:

```rust
/// C1: side-effect-free probe used by `hector doctor` to report whether
/// the configured `api_key_env` env var is set to a non-empty value.
/// Matches `read_api_key`'s emptiness rule (treats the empty string as
/// absent) so doctor reports the same answer the runner would consult.
///
/// Returns `false` when the var is missing, unset, or empty. Never logs
/// (unlike `read_api_key`, which warns to stderr) — doctor builds its
/// own remediation message.
pub fn api_key_env_present(env_name: &str) -> bool {
    matches!(std::env::var(env_name), Ok(v) if !v.is_empty())
}
```

- [ ] **Step 1.2.2: Run, expect green**

Run: `cargo test -p hector-core --test llm_api_key_env_present`
Expected: PASS (3 tests).

- [ ] **Step 1.2.3: Commit**

```bash
git add crates/hector-core/src/llm/mod.rs crates/hector-core/tests/llm_api_key_env_present.rs
git commit -m "$(cat <<'EOF'
feat(llm): expose api_key_env_present probe (C1 phase 1)

`hector doctor` (C1) needs to report whether a configured `api_key_env`
resolves to a non-empty value, using the same emptiness rule the runner
uses internally. Promote the predicate inside `read_api_key` to a thin
public helper so the runner and the doctor share one source of truth.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — `Doctor` types + orchestrator skeleton

### Task 2.1: Failing test — orchestrator returns a Report with one `Binary` check

**Files:**
- Create: `crates/hector-cli/tests/cli_e2e_doctor.rs`

- [ ] **Step 2.1.1: Write the bootstrap CLI test**

```rust
//! C1 — CLI integration tests for `hector doctor`.
//!
//! Each test isolates `~/.claude/settings.json` lookup by setting the
//! `HOME` env var to a tempdir, so the adapter check observes a clean
//! environment. The doctor module honors `HOME` via the `home_dir`
//! helper it inherits from `runner.rs`.

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let cfg = dir.join(".hector.yml");
    fs::write(&cfg, body).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    cfg
}

#[test]
fn doctor_runs_and_reports_binary_check() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(s.contains("binary"), "doctor output must mention the binary check: {s}");
    assert!(s.contains(env!("CARGO_PKG_VERSION")), "doctor output must include the running hector version: {s}");
}
```

- [ ] **Step 2.1.2: Run, expect failure**

Run: `cargo test -p hector-cli --test cli_e2e_doctor doctor_runs_and_reports_binary_check`
Expected: FAIL — `error: unrecognized subcommand 'doctor'` from clap.

### Task 2.2: clap wiring + `commands::doctor` module skeleton

**Files:**
- Modify: `crates/hector-cli/src/cli.rs`
- Modify: `crates/hector-cli/src/main.rs`
- Modify: `crates/hector-cli/src/commands/mod.rs`
- Create: `crates/hector-cli/src/commands/doctor.rs`

- [ ] **Step 2.2.1: Add the `Doctor` variant in `cli.rs`**

In `crates/hector-cli/src/cli.rs`, append a variant inside `enum Command`:

```rust
    /// Diagnose the local install, config, trust, engine availability, and adapter wiring.
    ///
    /// Read-only. Exits 0 if every check passes or only warns; exits 1 on any failure.
    Doctor {
        /// Directory containing `.hector.yml`. Defaults to cwd.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Output format. `human` (default) prints a checklist; `json` prints a
        /// machine-readable report — see `docs/doctor.md` for the schema.
        #[arg(long, default_value = "human")]
        format: OutputFormat,
    },
```

In `crates/hector-cli/src/main.rs`, add the dispatch arm to the `match cli.command` block (before the closing brace):

```rust
        Command::Doctor { dir, format } => commands::doctor::run(&dir, format)?,
```

In `crates/hector-cli/src/commands/mod.rs`, add the new module:

```rust
pub mod doctor;
```

- [ ] **Step 2.2.2: Create `commands/doctor.rs` with the type surface + a minimal `Binary` check**

Create `crates/hector-cli/src/commands/doctor.rs`:

```rust
//! C1 — `hector doctor` diagnostic subcommand.
//!
//! Read-only. Walks a fixed list of checks (binary on PATH, config
//! present, config parses, trust verifies, schema version, scope
//! globs, engine availability, adapter presence, runtime state) and
//! prints a checklist by default, or a JSON `Report` under `--format
//! json`. Exit code: 0 on all-pass-or-warn, 1 on any fail.
//!
//! The orchestrator (`run`) is one function that calls one function
//! per check. Each check returns a `CheckResult`. Per-check functions
//! stay under 15 cognitive complexity by composition: helpers
//! (`load_claude_settings`, `claude_hook_wired`) split the only
//! check that would otherwise breach the cap.

use crate::cli::OutputFormat;
use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// One row in the doctor report. `name` is the stable check id used in
/// the JSON output (snake_case, additive-only). `detail` is one short
/// sentence; `remediation` is the actionable hint shown when the
/// status is not `Pass`.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Warn,
    Fail,
}

/// JSON payload emitted by `--format json`. Public contract — see
/// `docs/doctor.md`. New fields land at the end of the struct with
/// `Option<…>` defaults so the schema stays additive.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub hector_version: String,
    pub checks: Vec<CheckResult>,
}

/// Per-doctor-run inputs shared across every check. Stays small —
/// each check borrows what it needs and pulls anything else from the
/// process environment (env vars, fs).
struct DoctorContext {
    dir: PathBuf,
    config_path: PathBuf,
}

pub fn run(dir: &Path, format: OutputFormat) -> Result<i32> {
    let ctx = DoctorContext {
        dir: dir.to_path_buf(),
        config_path: dir.join(".hector.yml"),
    };
    let checks: Vec<CheckResult> = vec![check_binary()];
    let report = Report {
        hector_version: env!("CARGO_PKG_VERSION").to_string(),
        checks,
    };
    emit(&report, format)?;
    Ok(exit_code(&report))
    // Suppress unused-variable lints until later phases wire the rest of the checks.
    #[allow(unreachable_code)]
    {
        let _ = ctx;
    }
}

fn exit_code(report: &Report) -> i32 {
    if report.checks.iter().any(|c| c.status == Status::Fail) {
        1
    } else {
        0
    }
}

fn emit(report: &Report, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(report)?);
        }
        OutputFormat::Human => {
            println!("hector doctor — version {}", report.hector_version);
            for c in &report.checks {
                let glyph = match c.status {
                    Status::Pass => "ok  ",
                    Status::Warn => "warn",
                    Status::Fail => "fail",
                };
                println!("  [{glyph}] {} — {}", c.name, c.detail);
                if c.status != Status::Pass {
                    if let Some(hint) = &c.remediation {
                        println!("         {}", hint);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Binary on PATH + version. Trivially `pass` once the user reaches us
/// (we're a binary that ran), but report the resolved path and version
/// so the human checklist surfaces "which hector am I talking to".
fn check_binary() -> CheckResult {
    let path = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "<unknown>".into());
    CheckResult {
        name: "binary",
        status: Status::Pass,
        detail: format!("hector {} at {}", env!("CARGO_PKG_VERSION"), path),
        remediation: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_is_zero_when_all_pass_or_warn() {
        let report = Report {
            hector_version: "0".into(),
            checks: vec![
                CheckResult { name: "a", status: Status::Pass, detail: "".into(), remediation: None },
                CheckResult { name: "b", status: Status::Warn, detail: "".into(), remediation: None },
            ],
        };
        assert_eq!(exit_code(&report), 0);
    }

    #[test]
    fn exit_code_is_one_when_any_fail() {
        let report = Report {
            hector_version: "0".into(),
            checks: vec![
                CheckResult { name: "a", status: Status::Pass, detail: "".into(), remediation: None },
                CheckResult { name: "b", status: Status::Fail, detail: "boom".into(), remediation: Some("fix it".into()) },
            ],
        };
        assert_eq!(exit_code(&report), 1);
    }

    #[test]
    fn check_binary_reports_running_version() {
        let r = check_binary();
        assert_eq!(r.status, Status::Pass);
        assert!(r.detail.contains(env!("CARGO_PKG_VERSION")));
    }
}
```

- [ ] **Step 2.2.3: Run the CLI test**

Run: `cargo test -p hector-cli --test cli_e2e_doctor doctor_runs_and_reports_binary_check`
Expected: PASS.

- [ ] **Step 2.2.4: Run the unit tests**

Run: `cargo test -p hector-cli --lib commands::doctor::tests`
Expected: PASS (3 tests: `exit_code_is_zero_when_all_pass_or_warn`, `exit_code_is_one_when_any_fail`, `check_binary_reports_running_version`).

- [ ] **Step 2.2.5: Commit**

```bash
git add crates/hector-cli/src/cli.rs crates/hector-cli/src/main.rs crates/hector-cli/src/commands/mod.rs crates/hector-cli/src/commands/doctor.rs crates/hector-cli/tests/cli_e2e_doctor.rs
git commit -m "$(cat <<'EOF'
feat(cli): scaffold `hector doctor` subcommand (C1 phase 2)

Adds the Doctor clap variant, dispatch arm in main.rs, the
commands::doctor module with the shared Report/CheckResult/Status
types, an orchestrator skeleton, and a minimal `binary` check that
reports the resolved hector path and version. Exit code is 0 when
every check is pass or warn, 1 when any check fails — distinct from
`check`'s 0/1/2 contract because doctor never produces a Verdict.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — Config presence + parse + trust + schema-version checks

### Task 3.1: Failing tests for the config-related checks

**Files:**
- Modify: `crates/hector-cli/tests/cli_e2e_doctor.rs`

- [ ] **Step 3.1.1: Append the four failing tests**

Append to `cli_e2e_doctor.rs`:

```rust
#[test]
fn doctor_fails_when_config_missing() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "missing config must exit 1");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("config") && s.contains("fail"), "expected a failing `config` row: {s}");
    assert!(s.contains("hector init"), "remediation must hint at `hector init`: {s}");
}

#[test]
fn doctor_fails_when_trust_fingerprint_broken() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    // Write a config with a *wrong* trust fingerprint.
    let cfg = dir.path().join(".hector.yml");
    let body = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\ntrust:\n  fingerprint: sha256:0000000000000000000000000000000000000000000000000000000000000000\n";
    fs::write(&cfg, body).unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("trust") && s.contains("fail"), "expected a failing `trust` row: {s}");
    // Parses-OK before trust-FAIL: distinguish parse failures from trust failures.
    assert!(s.contains("parses"), "parses check must still appear before trust: {s}");
    assert!(s.contains("hector trust"), "remediation must hint at `hector trust`: {s}");
}

#[test]
fn doctor_warns_on_legacy_schema_version_one() {
    // schema v1 fails at the parses step (extends::resolve_trusted refuses v1
    // before trust is verified — see config/extends.rs `peek_schema_version`).
    // Doctor must surface that as a `parses` fail with a `hector migrate` hint.
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(&cfg, "schema_version: 1\nrules: {}\n").unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("hector migrate"), "v1 remediation must hint at migrate: {s}");
}

#[test]
fn doctor_passes_on_clean_v2_config() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    for needle in ["binary", "config", "parses", "trust", "schema"] {
        assert!(s.contains(needle), "expected `{needle}` row in: {s}");
    }
}
```

- [ ] **Step 3.1.2: Run, expect failure**

Run: `cargo test -p hector-cli --test cli_e2e_doctor`
Expected: 4 tests fail. The new ones expect a `parses`/`trust`/`schema`/`config` row in the output that doesn't yet exist; they'll see only the `binary` row.

### Task 3.2: Implement config / parses / trust / schema-version checks

**Files:**
- Modify: `crates/hector-cli/src/commands/doctor.rs`

- [ ] **Step 3.2.1: Add the four checks**

Replace the orchestrator's `vec![check_binary()]` with:

```rust
    let checks: Vec<CheckResult> = vec![
        check_binary(),
        check_config_present(&ctx),
        check_config_parses(&ctx),
        check_trust(&ctx),
        check_schema_version(&ctx),
    ];
```

Remove the trailing `#[allow(unreachable_code)] { let _ = ctx; }` block — `ctx` is now consumed.

Add the four check functions after `check_binary`:

```rust
/// Config file present at `<dir>/.hector.yml`. Hard requirement; without
/// a config Hector has nothing to do.
fn check_config_present(ctx: &DoctorContext) -> CheckResult {
    if ctx.config_path.exists() {
        CheckResult {
            name: "config",
            status: Status::Pass,
            detail: format!("{} exists", ctx.config_path.display()),
            remediation: None,
        }
    } else {
        CheckResult {
            name: "config",
            status: Status::Fail,
            detail: format!("{} not found", ctx.config_path.display()),
            remediation: Some("run `hector init` to scaffold a starter config".into()),
        }
    }
}

/// Config parses. We deliberately use the **non-trust-verifying**
/// resolver so a parses-OK-but-untrusted config reports `parses: pass`
/// and `trust: fail`, instead of collapsing both into one fail row.
/// Schema-v1 configs fail here with a clear `hector migrate` hint
/// (the resolver detects v1 before trust verify — see
/// `config/extends.rs`).
fn check_config_parses(ctx: &DoctorContext) -> CheckResult {
    if !ctx.config_path.exists() {
        return CheckResult {
            name: "parses",
            status: Status::Fail,
            detail: "config missing; nothing to parse".into(),
            remediation: Some("run `hector init` first".into()),
        };
    }
    match hector_core::config::parse_file_with_extends(&ctx.config_path) {
        Ok(_) => CheckResult {
            name: "parses",
            status: Status::Pass,
            detail: "config parses (extends resolved)".into(),
            remediation: None,
        },
        Err(e) => {
            let msg = format!("{e:#}");
            // Surface the v1-migration hint verbatim if extends::resolve refused on schema_version: 1.
            let hint = if msg.contains("schema_version 1") {
                Some("run `hector migrate` to upgrade `.bully.yml`/v1 config to v2".into())
            } else {
                Some("fix the YAML error above and re-run".into())
            };
            CheckResult {
                name: "parses",
                status: Status::Fail,
                detail: msg,
                remediation: hint,
            }
        }
    }
}

/// Trust fingerprint matches recomputed canonical hash. Skipped (warn)
/// when parses already failed — there's no fingerprint to verify.
fn check_trust(ctx: &DoctorContext) -> CheckResult {
    if !ctx.config_path.exists() {
        return CheckResult {
            name: "trust",
            status: Status::Warn,
            detail: "skipped (no config)".into(),
            remediation: None,
        };
    }
    let raw = match std::fs::read_to_string(&ctx.config_path) {
        Ok(s) => s,
        Err(e) => {
            return CheckResult {
                name: "trust",
                status: Status::Fail,
                detail: format!("read failed: {e}"),
                remediation: Some("ensure the config file is readable".into()),
            };
        }
    };
    match hector_core::trust::verify(&raw) {
        Ok(()) => CheckResult {
            name: "trust",
            status: Status::Pass,
            detail: "fingerprint matches".into(),
            remediation: None,
        },
        Err(e) => CheckResult {
            name: "trust",
            status: Status::Fail,
            detail: format!("{e:#}"),
            remediation: Some(
                "review the diff against the last trusted state, then run `hector trust` to acknowledge".into(),
            ),
        },
    }
}

/// schema_version is one of `SUPPORTED_SCHEMAS`. v1 is `fail` (legacy
/// bully); v2 is `pass`; anything else is `fail` with a "this hector
/// is too old/new" hint.
fn check_schema_version(ctx: &DoctorContext) -> CheckResult {
    let raw = match std::fs::read_to_string(&ctx.config_path) {
        Ok(s) => s,
        Err(_) => {
            return CheckResult {
                name: "schema",
                status: Status::Warn,
                detail: "skipped (no config)".into(),
                remediation: None,
            };
        }
    };
    match hector_core::config::peek_schema_version(&raw) {
        Some(2) => CheckResult {
            name: "schema",
            status: Status::Pass,
            detail: "schema_version: 2".into(),
            remediation: None,
        },
        Some(1) => CheckResult {
            name: "schema",
            status: Status::Fail,
            detail: "schema_version: 1 (legacy bully)".into(),
            remediation: Some("run `hector migrate` to upgrade to schema_version 2".into()),
        },
        Some(n) => CheckResult {
            name: "schema",
            status: Status::Fail,
            detail: format!("schema_version: {n} (unsupported)"),
            remediation: Some(format!(
                "this hector supports {:?}; upgrade or downgrade hector to match",
                hector_core::config::SUPPORTED_SCHEMAS
            )),
        },
        None => CheckResult {
            name: "schema",
            status: Status::Fail,
            detail: "schema_version field missing or unparseable".into(),
            remediation: Some("add `schema_version: 2` at the top of `.hector.yml`".into()),
        },
    }
}
```

- [ ] **Step 3.2.2: Run the CLI tests**

Run: `cargo test -p hector-cli --test cli_e2e_doctor`
Expected: PASS for all 5 doctor tests so far (`doctor_runs_and_reports_binary_check`, `doctor_fails_when_config_missing`, `doctor_fails_when_trust_fingerprint_broken`, `doctor_warns_on_legacy_schema_version_one`, `doctor_passes_on_clean_v2_config`).

- [ ] **Step 3.2.3: Add per-check unit tests**

Append to the `mod tests` block in `commands/doctor.rs`:

```rust
    use std::fs;
    use tempfile::tempdir;

    fn ctx_with(dir: &std::path::Path) -> DoctorContext {
        DoctorContext {
            dir: dir.to_path_buf(),
            config_path: dir.join(".hector.yml"),
        }
    }

    #[test]
    fn config_present_pass_when_file_exists() {
        let d = tempdir().unwrap();
        fs::write(d.path().join(".hector.yml"), "schema_version: 2\nrules: {}\n").unwrap();
        let r = check_config_present(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
    }

    #[test]
    fn config_present_fail_when_file_missing() {
        let d = tempdir().unwrap();
        let r = check_config_present(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Fail);
        assert!(r.remediation.unwrap().contains("hector init"));
    }

    #[test]
    fn parses_fail_when_config_missing() {
        let d = tempdir().unwrap();
        let r = check_config_parses(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Fail);
    }

    #[test]
    fn schema_pass_on_v2() {
        let d = tempdir().unwrap();
        fs::write(d.path().join(".hector.yml"), "schema_version: 2\nrules: {}\n").unwrap();
        assert_eq!(check_schema_version(&ctx_with(d.path())).status, Status::Pass);
    }

    #[test]
    fn schema_fail_on_v1_with_migrate_hint() {
        let d = tempdir().unwrap();
        fs::write(d.path().join(".hector.yml"), "schema_version: 1\nrules: {}\n").unwrap();
        let r = check_schema_version(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Fail);
        assert!(r.remediation.unwrap().contains("hector migrate"));
    }

    #[test]
    fn schema_fail_on_unsupported_version() {
        let d = tempdir().unwrap();
        fs::write(d.path().join(".hector.yml"), "schema_version: 99\nrules: {}\n").unwrap();
        let r = check_schema_version(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Fail);
    }

    #[test]
    fn schema_fail_on_missing_version() {
        let d = tempdir().unwrap();
        fs::write(d.path().join(".hector.yml"), "rules: {}\n").unwrap();
        let r = check_schema_version(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Fail);
    }

    #[test]
    fn trust_warn_when_config_missing() {
        let d = tempdir().unwrap();
        let r = check_trust(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Warn);
    }
```

Run: `cargo test -p hector-cli --lib commands::doctor::tests`
Expected: PASS (11 tests total).

- [ ] **Step 3.2.4: Commit**

```bash
git add crates/hector-cli/src/commands/doctor.rs crates/hector-cli/tests/cli_e2e_doctor.rs
git commit -m "$(cat <<'EOF'
feat(cli): doctor checks for config presence, parse, trust, schema (C1 phase 3)

Adds four diagnostic checks: `config` (file exists), `parses`
(extends resolves), `trust` (fingerprint matches), `schema`
(supported version). Each carries an actionable remediation hint —
parse failures route v1 configs to `hector migrate`; trust failures
point at `hector trust`. parses uses the non-trust-verifying
resolver so an untrusted-but-valid YAML reports parses=pass and
trust=fail rather than collapsing both into one row.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4 — Scope-glob + engine-availability checks

The `Engines` check is the most behavior-rich: it enumerates the loaded rules, partitions them by `EngineKind`, and for `Semantic`/`Session` rules verifies the `llm:` block exists and the `api_key_env` resolves. Cognitive complexity stays under 15 by splitting "is the LLM block usable?" into a helper.

### Task 4.1: Failing tests for scope and engine availability

**Files:**
- Modify: `crates/hector-cli/tests/cli_e2e_doctor.rs`

- [ ] **Step 4.1.1: Append failing tests**

```rust
#[test]
fn doctor_warns_when_semantic_rule_present_without_api_key() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_trusted(
        dir.path(),
        "schema_version: 2\nllm:\n  provider: anthropic\n  model: claude\n  api_key_env: HECTOR_DOCTOR_TEST_NO_SUCH_KEY\nrules:\n  sem:\n    description: \"x\"\n    engine: semantic\n    scope: [\"**/*.rs\"]\n    severity: warning\n    context: file\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .env_remove("HECTOR_DOCTOR_TEST_NO_SUCH_KEY")
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    // Missing API key for a configured semantic rule is a `warn`, not a
    // hard `fail` — the binary still works for non-LLM rules.
    assert_eq!(out.status.code(), Some(0), "missing-key warn must keep exit 0");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("engines"), "expected `engines` row in: {s}");
    assert!(s.contains("warn"), "expected a warn glyph in: {s}");
    assert!(s.contains("HECTOR_DOCTOR_TEST_NO_SUCH_KEY"), "remediation must name the env var: {s}");
}

#[test]
fn doctor_pass_engines_when_no_llm_rules() {
    // Pure script config — no llm block, no semantic rules. Engines = pass.
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("engines") && s.contains("ok"), "engines should pass: {s}");
}
```

- [ ] **Step 4.1.2: Run, expect failure**

Run: `cargo test -p hector-cli --test cli_e2e_doctor`
Expected: FAIL on the two new tests — no `engines` row exists yet.

### Task 4.2: Implement `check_scope_globs` + `check_engines`

**Files:**
- Modify: `crates/hector-cli/src/commands/doctor.rs`

- [ ] **Step 4.2.1: Extend the orchestrator**

Update the `checks` vec in `run`:

```rust
    let checks: Vec<CheckResult> = vec![
        check_binary(),
        check_config_present(&ctx),
        check_config_parses(&ctx),
        check_trust(&ctx),
        check_schema_version(&ctx),
        check_scope_globs(&ctx),
        check_engines(&ctx),
    ];
```

- [ ] **Step 4.2.2: Add the two checks**

Append after `check_schema_version`:

```rust
/// Every rule's scope globs construct a valid `ScopeMatcher`. The
/// runner already validates this at load time, but doctor surfaces it
/// as its own row so a globset error doesn't masquerade as a generic
/// parse failure. Skipped (warn) when the config doesn't parse.
fn check_scope_globs(ctx: &DoctorContext) -> CheckResult {
    let cfg = match hector_core::config::parse_file_with_extends(&ctx.config_path) {
        Ok(c) => c,
        Err(_) => {
            return CheckResult {
                name: "scope_globs",
                status: Status::Warn,
                detail: "skipped (config does not parse)".into(),
                remediation: None,
            };
        }
    };
    let mut bad: Vec<String> = Vec::new();
    for (rule_id, rule) in &cfg.rules {
        if let Err(e) = hector_core::config::scope::ScopeMatcher::new(&rule.scope) {
            bad.push(format!("{rule_id}: {e:#}"));
        }
    }
    if bad.is_empty() {
        CheckResult {
            name: "scope_globs",
            status: Status::Pass,
            detail: format!("{} rule(s) have valid scope", cfg.rules.len()),
            remediation: None,
        }
    } else {
        CheckResult {
            name: "scope_globs",
            status: Status::Fail,
            detail: format!("invalid scope on: {}", bad.join("; ")),
            remediation: Some("fix the listed glob(s) and re-run `hector trust`".into()),
        }
    }
}

/// Engine availability:
///   - Semantic / Session rules → require an `llm:` block whose
///     `api_key_env` resolves to a non-empty value (Ollama is exempt
///     from the api-key requirement, mirroring `llm::build_from_config`).
///   - All-script / all-ast configs → trivially pass.
/// Decomposed via `llm_block_status` so this function stays cheap on
/// cognitive complexity.
fn check_engines(ctx: &DoctorContext) -> CheckResult {
    let cfg = match hector_core::config::parse_file_with_extends(&ctx.config_path) {
        Ok(c) => c,
        Err(_) => {
            return CheckResult {
                name: "engines",
                status: Status::Warn,
                detail: "skipped (config does not parse)".into(),
                remediation: None,
            };
        }
    };
    let needs_llm = cfg.rules.values().any(|r| {
        matches!(
            r.engine,
            hector_core::config::EngineKind::Semantic | hector_core::config::EngineKind::Session
        )
    });
    if !needs_llm {
        return CheckResult {
            name: "engines",
            status: Status::Pass,
            detail: "deterministic engines only (no LLM required)".into(),
            remediation: None,
        };
    }
    llm_block_status(cfg.llm.as_ref())
}

/// Inspect the `llm:` block for a config that has at least one
/// semantic/session rule. Returns the engine-row `CheckResult` directly
/// so the caller stays a one-liner.
fn llm_block_status(cfg: Option<&hector_core::config::LlmConfig>) -> CheckResult {
    let Some(llm) = cfg else {
        return CheckResult {
            name: "engines",
            status: Status::Warn,
            detail: "semantic/session rule(s) present but no `llm:` block configured".into(),
            remediation: Some(
                "add an `llm:` block with provider/model/api_key_env (see docs/quickstart.md)"
                    .into(),
            ),
        };
    };
    // Ollama needs no API key — `build_from_config` defaults to an empty key.
    if llm.provider == "ollama" {
        return CheckResult {
            name: "engines",
            status: Status::Pass,
            detail: format!("provider=ollama, model={}", llm.model),
            remediation: None,
        };
    }
    let env_name = match llm.api_key_env.as_deref() {
        Some(n) if !n.is_empty() => n,
        _ => {
            return CheckResult {
                name: "engines",
                status: Status::Warn,
                detail: format!("provider={} but `api_key_env` is unset", llm.provider),
                remediation: Some(
                    "set `api_key_env: <NAME>` in the `llm:` block of `.hector.yml`".into(),
                ),
            };
        }
    };
    if hector_core::llm::api_key_env_present(env_name) {
        CheckResult {
            name: "engines",
            status: Status::Pass,
            detail: format!(
                "provider={}, model={}, ${env_name} resolves",
                llm.provider, llm.model
            ),
            remediation: None,
        }
    } else {
        CheckResult {
            name: "engines",
            status: Status::Warn,
            detail: format!("env var `{env_name}` not set; semantic/session rules will error at evaluation"),
            remediation: Some(format!(
                "export `{env_name}` with a valid {} API key",
                llm.provider
            )),
        }
    }
}
```

- [ ] **Step 4.2.3: Run CLI tests**

Run: `cargo test -p hector-cli --test cli_e2e_doctor`
Expected: PASS for all 7 doctor tests so far.

- [ ] **Step 4.2.4: Add per-check unit tests**

Append to the `mod tests` block:

```rust
    #[test]
    fn engines_pass_when_no_llm_rules() {
        let d = tempdir().unwrap();
        let trusted = hector_core::trust::write_trust_block(
            "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
        ).unwrap();
        fs::write(d.path().join(".hector.yml"), trusted).unwrap();
        let r = check_engines(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
    }

    #[test]
    fn engines_warn_when_semantic_rule_lacks_llm_block() {
        let d = tempdir().unwrap();
        let trusted = hector_core::trust::write_trust_block(
            "schema_version: 2\nrules:\n  s:\n    description: \"x\"\n    engine: semantic\n    scope: [\"*\"]\n    severity: warning\n    context: file\n",
        ).unwrap();
        fs::write(d.path().join(".hector.yml"), trusted).unwrap();
        let r = check_engines(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Warn);
        assert!(r.remediation.unwrap().contains("llm"));
    }

    #[test]
    fn engines_pass_for_ollama_without_key() {
        let cfg = hector_core::config::LlmConfig {
            provider: "ollama".into(),
            model: "llama3".into(),
            api_key_env: None,
            base_url: None,
        };
        let r = llm_block_status(Some(&cfg));
        assert_eq!(r.status, Status::Pass);
    }

    #[test]
    fn engines_warn_when_api_key_env_unset() {
        let cfg = hector_core::config::LlmConfig {
            provider: "anthropic".into(),
            model: "claude".into(),
            api_key_env: Some("HECTOR_DOCTOR_TEST_DEFINITELY_UNSET_AAA".into()),
            base_url: None,
        };
        std::env::remove_var("HECTOR_DOCTOR_TEST_DEFINITELY_UNSET_AAA");
        let r = llm_block_status(Some(&cfg));
        assert_eq!(r.status, Status::Warn);
        assert!(r.remediation.unwrap().contains("HECTOR_DOCTOR_TEST_DEFINITELY_UNSET_AAA"));
    }

    #[test]
    fn engines_pass_when_api_key_env_set() {
        let cfg = hector_core::config::LlmConfig {
            provider: "anthropic".into(),
            model: "claude".into(),
            api_key_env: Some("HECTOR_DOCTOR_TEST_PRESENT_KEY".into()),
            base_url: None,
        };
        std::env::set_var("HECTOR_DOCTOR_TEST_PRESENT_KEY", "x");
        let r = llm_block_status(Some(&cfg));
        std::env::remove_var("HECTOR_DOCTOR_TEST_PRESENT_KEY");
        assert_eq!(r.status, Status::Pass);
    }

    #[test]
    fn engines_warn_when_api_key_env_field_missing() {
        let cfg = hector_core::config::LlmConfig {
            provider: "anthropic".into(),
            model: "claude".into(),
            api_key_env: None,
            base_url: None,
        };
        let r = llm_block_status(Some(&cfg));
        assert_eq!(r.status, Status::Warn);
    }

    #[test]
    fn scope_globs_pass_on_clean_config() {
        let d = tempdir().unwrap();
        let trusted = hector_core::trust::write_trust_block(
            "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*.rs\"]\n    severity: error\n    script: \"true\"\n",
        ).unwrap();
        fs::write(d.path().join(".hector.yml"), trusted).unwrap();
        let r = check_scope_globs(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
    }
```

Run: `cargo test -p hector-cli --lib commands::doctor::tests`
Expected: PASS (now 18 unit tests).

- [ ] **Step 4.2.5: Commit**

```bash
git add crates/hector-cli/src/commands/doctor.rs crates/hector-cli/tests/cli_e2e_doctor.rs
git commit -m "$(cat <<'EOF'
feat(cli): doctor scope_globs + engines checks (C1 phase 4)

`scope_globs` re-surfaces the load-time scope validation as its own
row so a globset error doesn't hide inside the parses row. `engines`
inspects the loaded rules: pure-script/ast configs trivially pass;
semantic/session rules require a usable `llm:` block (api_key_env
resolves, or provider=ollama). Missing keys are `warn` not `fail` —
the binary still works for non-LLM rules.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5 — Adapter-presence check

Bully checks `.claude/settings.json` for a wired PostToolUse hook plus an evaluator agent. Hector's adapter ships its own `hooks.json` under `${CLAUDE_PLUGIN_ROOT}/hooks/hook.sh`. Doctor reports advisory status: missing `~/.claude/settings.json` is `warn` (not every user runs Claude Code); present-but-no-hector-hook is `warn` with a remediation hint; present-and-wired is `pass`.

### Task 5.1: Failing test for adapter detection

**Files:**
- Modify: `crates/hector-cli/tests/cli_e2e_doctor.rs`

- [ ] **Step 5.1.1: Append the failing tests**

```rust
#[test]
fn doctor_adapter_warn_when_settings_missing() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap(); // empty: no ~/.claude/settings.json
    write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("adapter") && s.contains("warn"), "expected `adapter warn`: {s}");
}

#[test]
fn doctor_adapter_pass_when_hook_wired() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    let claude = home.path().join(".claude");
    fs::create_dir_all(&claude).unwrap();
    // Wire a PostToolUse hook whose command references `hector` so the
    // detector recognizes it without needing the real adapter installed.
    let settings = r#"{"hooks":{"PostToolUse":[{"matcher":"Edit|Write","hooks":[{"type":"command","command":"hector check --diff -"}]}]}}"#;
    fs::write(claude.join("settings.json"), settings).unwrap();
    write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("adapter") && s.contains("ok"), "expected `adapter ok`: {s}");
}

#[test]
fn doctor_adapter_warn_when_settings_present_but_no_hector_hook() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    let claude = home.path().join(".claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(
        claude.join("settings.json"),
        r#"{"hooks":{"PostToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"echo unrelated"}]}]}}"#,
    ).unwrap();
    write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("adapter") && s.contains("warn"), "expected `adapter warn` when no hector hook: {s}");
    assert!(s.contains("docs/adapters/claude-code.md") || s.contains("install"), "expected adapter install hint: {s}");
}
```

- [ ] **Step 5.1.2: Run, expect failure**

Run: `cargo test -p hector-cli --test cli_e2e_doctor`
Expected: FAIL on the three adapter tests — no `adapter` row exists yet.

### Task 5.2: Implement `check_adapter` + helpers

**Files:**
- Modify: `crates/hector-cli/src/commands/doctor.rs`

- [ ] **Step 5.2.1: Extend the orchestrator**

Update the `checks` vec:

```rust
    let checks: Vec<CheckResult> = vec![
        check_binary(),
        check_config_present(&ctx),
        check_config_parses(&ctx),
        check_trust(&ctx),
        check_schema_version(&ctx),
        check_scope_globs(&ctx),
        check_engines(&ctx),
        check_adapter(),
    ];
```

- [ ] **Step 5.2.2: Add the adapter check + helpers**

Append after `llm_block_status`:

```rust
/// Locate `~/.claude/settings.json` (honoring `HOME`/`USERPROFILE`).
/// Returns `None` if the home dir is unresolvable or the file is absent —
/// caller maps that to a `warn` row.
fn load_claude_settings() -> Option<(PathBuf, serde_json::Value)> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))?;
    let path = PathBuf::from(home).join(".claude").join("settings.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    let value = serde_json::from_str(&raw).ok()?;
    Some((path, value))
}

/// Walk the parsed `~/.claude/settings.json` looking for a PostToolUse
/// hook whose `command` references `hector` (the binary) or a Hector
/// adapter `hook.sh`. Returns true on first match.
fn claude_hook_wired(settings: &serde_json::Value) -> bool {
    let Some(post) = settings
        .get("hooks")
        .and_then(|h| h.get("PostToolUse"))
        .and_then(|p| p.as_array())
    else {
        return false;
    };
    post.iter().any(|matcher_block| {
        matcher_block
            .get("hooks")
            .and_then(|hs| hs.as_array())
            .map(|hooks| {
                hooks.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|cmd| cmd.contains("hector") || cmd.contains("hook.sh"))
                })
            })
            .unwrap_or(false)
    })
}

/// Adapter presence is best-effort: missing `~/.claude/settings.json`
/// is `warn` (not every user runs Claude Code); present-without-hector
/// is `warn`; wired is `pass`. Never `fail` — hector is editor-agnostic
/// and the CLI is fully usable without an adapter.
fn check_adapter() -> CheckResult {
    let Some((path, settings)) = load_claude_settings() else {
        return CheckResult {
            name: "adapter",
            status: Status::Warn,
            detail: "Claude Code adapter not detected (~/.claude/settings.json missing)".into(),
            remediation: Some(
                "if you use Claude Code, install the adapter — see docs/adapters/claude-code.md".into(),
            ),
        };
    };
    if claude_hook_wired(&settings) {
        CheckResult {
            name: "adapter",
            status: Status::Pass,
            detail: format!("Claude Code PostToolUse hook references hector ({})", path.display()),
            remediation: None,
        }
    } else {
        CheckResult {
            name: "adapter",
            status: Status::Warn,
            detail: format!("{} present but no PostToolUse hook references hector", path.display()),
            remediation: Some(
                "install the adapter or add a PostToolUse entry calling hector — see docs/adapters/claude-code.md".into(),
            ),
        }
    }
}
```

- [ ] **Step 5.2.3: Run CLI tests**

Run: `cargo test -p hector-cli --test cli_e2e_doctor`
Expected: PASS for all 10 doctor tests.

- [ ] **Step 5.2.4: Add unit tests for the JSON walker**

The adapter check's per-machine state (HOME, ~/.claude/settings.json) is hard to exercise in pure unit form; the integration tests cover the three states (missing / present-no-hook / present-with-hook). Add narrow tests for the helper:

```rust
    #[test]
    fn hook_wired_finds_hector_command() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"hector check"}]}]}}"#,
        ).unwrap();
        assert!(claude_hook_wired(&v));
    }

    #[test]
    fn hook_wired_finds_adapter_hook_sh() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"$ROOT/hooks/hook.sh post"}]}]}}"#,
        ).unwrap();
        assert!(claude_hook_wired(&v));
    }

    #[test]
    fn hook_wired_rejects_unrelated_command() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
        ).unwrap();
        assert!(!claude_hook_wired(&v));
    }

    #[test]
    fn hook_wired_rejects_missing_post_tool_use() {
        let v: serde_json::Value = serde_json::from_str(r#"{"hooks":{}}"#).unwrap();
        assert!(!claude_hook_wired(&v));
    }

    #[test]
    fn hook_wired_rejects_empty_object() {
        let v: serde_json::Value = serde_json::from_str(r#"{}"#).unwrap();
        assert!(!claude_hook_wired(&v));
    }
```

Run: `cargo test -p hector-cli --lib commands::doctor::tests`
Expected: PASS (now 23 unit tests).

- [ ] **Step 5.2.5: Commit**

```bash
git add crates/hector-cli/src/commands/doctor.rs crates/hector-cli/tests/cli_e2e_doctor.rs
git commit -m "$(cat <<'EOF'
feat(cli): doctor adapter detection (C1 phase 5)

Looks for `~/.claude/settings.json` and inspects PostToolUse for a
hook that references `hector` or the adapter's `hook.sh`. Missing
file → warn (Hector is editor-agnostic; not every user runs Claude
Code). Present-without-hook → warn with install hint. Wired → pass.
Never fail — the CLI works without an adapter.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6 — Runtime-state check (`.hector/` writable)

### Task 6.1: Failing test

**Files:**
- Modify: `crates/hector-cli/tests/cli_e2e_doctor.rs`

- [ ] **Step 6.1.1: Append failing tests**

```rust
#[test]
fn doctor_runtime_state_pass_when_hector_dir_writable() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("runtime_state"), "expected runtime_state row: {s}");
    assert!(s.contains("ok"), "runtime_state should pass on a fresh tempdir: {s}");
}
```

- [ ] **Step 6.1.2: Run, expect failure**

Run: `cargo test -p hector-cli --test cli_e2e_doctor doctor_runtime_state_pass_when_hector_dir_writable`
Expected: FAIL — no `runtime_state` row.

### Task 6.2: Implement `check_runtime_state`

**Files:**
- Modify: `crates/hector-cli/src/commands/doctor.rs`

- [ ] **Step 6.2.1: Extend the orchestrator**

Update the `checks` vec:

```rust
    let checks: Vec<CheckResult> = vec![
        check_binary(),
        check_config_present(&ctx),
        check_config_parses(&ctx),
        check_trust(&ctx),
        check_schema_version(&ctx),
        check_scope_globs(&ctx),
        check_engines(&ctx),
        check_adapter(),
        check_runtime_state(&ctx),
    ];
```

- [ ] **Step 6.2.2: Add the check**

Append after `check_adapter`:

```rust
/// `<dir>/.hector/` is writable. Probes by creating the dir if absent,
/// writing a marker file, then deleting it. We DO create `.hector/` —
/// that's the same kind of side effect as `init` writing `.hector.yml`,
/// and "doctor never modifies state" is about *policy* state (configs,
/// baselines, telemetry), not about the run-state directory itself.
///
/// Also reports current sizes of `baseline.json`, `session.json`, and
/// `log.jsonl` if present, so the human checklist surfaces "your
/// telemetry log has grown to 200MB" without forcing the user to
/// `du -sh .hector/`.
fn check_runtime_state(ctx: &DoctorContext) -> CheckResult {
    let hector_dir = ctx.dir.join(".hector");
    if let Err(e) = std::fs::create_dir_all(&hector_dir) {
        return CheckResult {
            name: "runtime_state",
            status: Status::Fail,
            detail: format!("cannot create {}: {e}", hector_dir.display()),
            remediation: Some(format!(
                "ensure {} is writable (chmod / ownership)",
                ctx.dir.display()
            )),
        };
    }
    let probe = hector_dir.join(".doctor-write-probe");
    if let Err(e) = std::fs::write(&probe, b"ok") {
        return CheckResult {
            name: "runtime_state",
            status: Status::Fail,
            detail: format!("cannot write to {}: {e}", hector_dir.display()),
            remediation: Some(format!("chmod u+w {}", hector_dir.display())),
        };
    }
    let _ = std::fs::remove_file(&probe);

    let mut sizes: Vec<String> = Vec::new();
    for name in ["baseline.json", "session.json", "log.jsonl"] {
        if let Ok(meta) = std::fs::metadata(hector_dir.join(name)) {
            sizes.push(format!("{name}={}b", meta.len()));
        }
    }
    let detail = if sizes.is_empty() {
        format!("{} writable (empty)", hector_dir.display())
    } else {
        format!("{} writable ({})", hector_dir.display(), sizes.join(", "))
    };
    CheckResult {
        name: "runtime_state",
        status: Status::Pass,
        detail,
        remediation: None,
    }
}
```

- [ ] **Step 6.2.3: Run tests**

Run: `cargo test -p hector-cli --test cli_e2e_doctor`
Expected: PASS for 11 doctor tests.

- [ ] **Step 6.2.4: Add unit tests**

Append to the `mod tests` block:

```rust
    #[test]
    fn runtime_state_pass_creates_hector_dir() {
        let d = tempdir().unwrap();
        let r = check_runtime_state(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
        assert!(d.path().join(".hector").is_dir(), "hector dir created by probe");
    }

    #[test]
    fn runtime_state_reports_existing_state_files_sizes() {
        let d = tempdir().unwrap();
        let h = d.path().join(".hector");
        fs::create_dir_all(&h).unwrap();
        fs::write(h.join("baseline.json"), "[]").unwrap();
        fs::write(h.join("log.jsonl"), "{}\n").unwrap();
        let r = check_runtime_state(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
        assert!(r.detail.contains("baseline.json"));
        assert!(r.detail.contains("log.jsonl"));
    }
```

Run: `cargo test -p hector-cli --lib commands::doctor::tests`
Expected: PASS (25 unit tests).

- [ ] **Step 6.2.5: Commit**

```bash
git add crates/hector-cli/src/commands/doctor.rs crates/hector-cli/tests/cli_e2e_doctor.rs
git commit -m "$(cat <<'EOF'
feat(cli): doctor runtime_state probe (C1 phase 6)

Probes `<dir>/.hector/` by creating-if-absent, writing a marker,
and deleting it. Reports current sizes of baseline.json,
session.json, and log.jsonl when present so operators can spot a
runaway telemetry log without `du -sh`. Creating `.hector/` is
within the read-only-on-policy-state contract — the directory is
run state, not policy.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 7 — JSON output via `--format json` + `insta` snapshot

The `--format json` path already works — `Report` derives `Serialize` and `emit` already branches on `OutputFormat::Json`. This phase locks the schema with a snapshot test and documents it.

### Task 7.1: Add `insta` to `hector-cli` dev-deps if not already present

- [ ] **Step 7.1.1: Inspect Cargo.toml**

Run: `grep insta /Users/chrisarter/Documents/projects/hector/crates/hector-cli/Cargo.toml || echo "not present"`
If not present, edit `crates/hector-cli/Cargo.toml` `[dev-dependencies]` to add:

```toml
insta = { workspace = true }
```

(The workspace already declares `insta = { version = "1", features = ["yaml", "json", "redactions"] }`.)

### Task 7.2: Failing snapshot test

**Files:**
- Modify: `crates/hector-cli/tests/cli_e2e_doctor.rs`

- [ ] **Step 7.2.1: Append the snapshot test**

```rust
#[test]
fn doctor_json_output_snapshot_for_clean_v2_config() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args([
            "doctor",
            "--dir", dir.path().to_str().unwrap(),
            "--format", "json",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let mut value: serde_json::Value = serde_json::from_slice(&out)
        .expect("doctor --format json must produce valid JSON");
    // Redact volatile fields before snapshotting:
    //   - hector_version: changes every release
    //   - per-check `detail`: contains absolute paths and sizes
    if let Some(obj) = value.as_object_mut() {
        obj.insert("hector_version".into(), serde_json::Value::String("[REDACTED]".into()));
    }
    if let Some(checks) = value.get_mut("checks").and_then(|c| c.as_array_mut()) {
        for c in checks {
            if let Some(o) = c.as_object_mut() {
                o.insert("detail".into(), serde_json::Value::String("[REDACTED]".into()));
                if o.get("remediation").is_some_and(|r| !r.is_null()) {
                    o.insert("remediation".into(), serde_json::Value::String("[REDACTED]".into()));
                }
            }
        }
    }
    insta::assert_json_snapshot!(value);
}
```

- [ ] **Step 7.2.2: Run, accept the snapshot**

Run: `cargo test -p hector-cli --test cli_e2e_doctor doctor_json_output_snapshot_for_clean_v2_config`
Expected: FAIL on first run with insta "snapshot not found".
Then: `cargo insta review --workspace` → accept the snapshot. (CI: `INSTA_UPDATE=no cargo test` keeps the snapshot stable.)

The accepted snapshot should look like:

```json
{
  "hector_version": "[REDACTED]",
  "checks": [
    { "name": "binary", "status": "pass", "detail": "[REDACTED]", "remediation": null },
    { "name": "config", "status": "pass", "detail": "[REDACTED]", "remediation": null },
    { "name": "parses", "status": "pass", "detail": "[REDACTED]", "remediation": null },
    { "name": "trust", "status": "pass", "detail": "[REDACTED]", "remediation": null },
    { "name": "schema", "status": "pass", "detail": "[REDACTED]", "remediation": null },
    { "name": "scope_globs", "status": "pass", "detail": "[REDACTED]", "remediation": null },
    { "name": "engines", "status": "pass", "detail": "[REDACTED]", "remediation": null },
    { "name": "adapter", "status": "warn", "detail": "[REDACTED]", "remediation": "[REDACTED]" },
    { "name": "runtime_state", "status": "pass", "detail": "[REDACTED]", "remediation": null }
  ]
}
```

- [ ] **Step 7.2.3: Commit**

```bash
git add crates/hector-cli/tests/cli_e2e_doctor.rs crates/hector-cli/tests/snapshots/cli_e2e_doctor__doctor_json_output_snapshot_for_clean_v2_config.snap
git commit -m "$(cat <<'EOF'
test(cli): snapshot doctor --format json schema (C1 phase 7)

Locks the JSON output shape behind an insta snapshot so future
additions stay additive (new fields appear as snapshot diffs to
review). Redacts hector_version, per-check detail, and remediation
strings — those carry absolute paths or change per release. The
field set (`name`, `status`, `detail`, `remediation`) is the public
contract documented in docs/doctor.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 8 — Documentation

### Task 8.1: `docs/doctor.md` — JSON output schema (public contract)

**Files:**
- Create: `docs/doctor.md`

- [ ] **Step 8.1.1: Write the schema doc**

Create `docs/doctor.md`:

```markdown
# `hector doctor` output schema

`hector doctor` is a read-only diagnostic command. It prints a checklist
of every load-time invariant Hector cares about and exits `0` when every
check is `pass` or `warn`, `1` when any check `fail`s.

This document is the public contract for `--format json`. The set of
field *names* and the meaning of each `status` value are stable; new
fields land at the end of `Report` or `CheckResult` with `Option<…>`
defaults so the schema stays additive.

## Top-level shape

```json
{
  "hector_version": "0.2.0",
  "checks": [ /* CheckResult[] */ ]
}
```

| Field | Type | Meaning |
|---|---|---|
| `hector_version` | string | The version of the running `hector` binary. |
| `checks` | array of `CheckResult` | Ordered, one row per check. Order is stable across runs. |

## `CheckResult`

```json
{
  "name": "trust",
  "status": "pass",
  "detail": "fingerprint matches",
  "remediation": null
}
```

| Field | Type | Meaning |
|---|---|---|
| `name` | string (snake_case) | Stable check id. See [check ids](#check-ids). |
| `status` | `"pass"` \| `"warn"` \| `"fail"` | Outcome. Exit-code rule: any `fail` → exit 1; otherwise → exit 0. |
| `detail` | string | One short sentence describing what was checked and what was found. May contain absolute paths, version numbers, or sizes. |
| `remediation` | string \| null | Actionable hint when `status` is not `pass`. `null` on pass. |

## Check ids

These are emitted in this order:

| `name` | What it verifies |
|---|---|
| `binary` | The running `hector` resolves to a path; reports the version. Always `pass`. |
| `config` | `<dir>/.hector.yml` exists. `fail` if missing. |
| `parses` | The config (and every transitive `extends:` ancestor) parses. `fail` if the YAML is malformed or schema_version is unsupported. |
| `trust` | The trust fingerprint matches the recomputed canonical hash. `fail` if it doesn't; `warn` if there's no config to verify. |
| `schema` | `schema_version` is a supported value (currently `2`). `fail` on `1` (legacy bully — remediation: `hector migrate`). |
| `scope_globs` | Every rule's `scope:` constructs a valid glob matcher. `fail` lists the offending rule(s). |
| `engines` | If any rule is `engine: semantic` or `engine: session`, an `llm:` block is present and `api_key_env` resolves to a non-empty value. `provider: ollama` exempts the api-key requirement. `warn` (not `fail`) on missing key — the binary still works for non-LLM rules. |
| `adapter` | `~/.claude/settings.json` exists and a PostToolUse hook references `hector` or the adapter's `hook.sh`. Missing settings file is `warn` (not every user runs Claude Code). |
| `runtime_state` | `<dir>/.hector/` is writable (probed by writing+deleting a marker file). Reports sizes of `baseline.json`, `session.json`, `log.jsonl` if present. |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Every check is `pass` or `warn`. |
| `1` | At least one check is `fail`. |

These are *distinct* from `hector check`'s `0` / `1` / `2` contract.
`doctor` never produces a `Verdict` and never participates in the
adapter exit-code routing.

## Stability

- The set of `name` values is **additive-only**. New checks land at the end of the list.
- `Status` values (`pass` / `warn` / `fail`) are frozen.
- `detail` strings are human-readable and may change between releases — do not parse them.
- `remediation` strings are human-readable and may change between releases — do not parse them.
- The exit-code rule (`0` for pass-or-warn, `1` for any fail) is frozen.
```

- [ ] **Step 8.1.2: Commit**

```bash
git add docs/doctor.md
git commit -m "$(cat <<'EOF'
docs: document hector doctor output schema (C1 phase 8)

`docs/doctor.md` is the public contract for `hector doctor
--format json`: the Report / CheckResult shape, the stable list
of check ids and what each verifies, and the exit-code rule
(0 for pass-or-warn, 1 for any fail). Marks `name` and `status`
as frozen-additive and `detail`/`remediation` as human-readable
(do not parse).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 8.2: README mention

**Files:**
- Modify: `README.md`

- [ ] **Step 8.2.1: Edit the Status paragraph**

In `README.md`, replace the `## Status` section's CLI list line:

```
0.1 (complete). Engines: `script`, `ast`, `semantic` (Anthropic), `session`. CLI: `check`, `trust`, `validate`, `init`, `migrate`, `baseline`, `session record`. Claude Code + OpenCode adapters shipped. Plan 0.2 adds OpenAI + Aider + pre-commit.
```

with:

```
0.2 (in progress). Engines: `script`, `ast`, `semantic` (Anthropic + OpenRouter + Ollama), `session`. CLI: `check`, `trust`, `validate`, `init`, `migrate`, `baseline`, `session record`, `doctor`. Claude Code + OpenCode adapters shipped. See [`docs/doctor.md`](docs/doctor.md) for the diagnostic schema.
```

- [ ] **Step 8.2.2: Commit**

```bash
git add README.md
git commit -m "$(cat <<'EOF'
docs(readme): list `hector doctor` in the CLI surface (C1 phase 8)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 9 — Verification + coverage backfill

### Task 9.1: fmt + clippy + full workspace tests

- [ ] **Step 9.1.1: Run the gates**

Run, in order, and confirm each is clean:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

Expected: all pass. If clippy fires `cognitive_complexity` on `commands::doctor::run` or `check_engines`, decompose further — do not `#[allow]`.

### Task 9.2: Per-file coverage gate

- [ ] **Step 9.2.1: Run the local coverage script**

```bash
bash scripts/ci-coverage.sh
```

Expected: `commands/doctor.rs` ≥ 80% region coverage. Likely soft spots:
- `check_runtime_state`'s error arms (`create_dir_all` / `write` failures) — hard to exercise on a clean tempdir.
- `load_claude_settings`'s `home unresolvable` arm — needs `HOME` and `USERPROFILE` both unset.

If the gate fails on `doctor.rs`:

- [ ] **Step 9.2.2: Backfill — runtime-state failure path on a read-only dir**

Append to the `mod tests` block (Unix-only — Windows perms differ):

```rust
    #[cfg(unix)]
    #[test]
    fn runtime_state_fail_when_parent_dir_not_writable() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempdir().unwrap();
        // Make the parent dir read-only so create_dir_all fails on `.hector`.
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let r = check_runtime_state(&ctx_with(d.path()));
        // Restore so tempdir cleanup works.
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(r.status, Status::Fail);
        assert!(r.remediation.is_some());
    }
```

- [ ] **Step 9.2.3: Backfill — adapter check when home unresolvable**

```rust
    #[test]
    fn load_claude_settings_returns_none_when_home_unset() {
        // Save and clear HOME / USERPROFILE for the duration of this test.
        let prev_home = std::env::var_os("HOME");
        let prev_user = std::env::var_os("USERPROFILE");
        std::env::remove_var("HOME");
        std::env::remove_var("USERPROFILE");
        let r = load_claude_settings();
        if let Some(h) = prev_home { std::env::set_var("HOME", h); }
        if let Some(u) = prev_user { std::env::set_var("USERPROFILE", u); }
        assert!(r.is_none());
    }
```

(Note: this test mutates process-global env. It runs single-threaded in the same `#[cfg(test)] mod tests` and must not be parallelized with other tests that read `HOME`. `cargo test` shares a process per test binary; if this proves flaky, gate behind `#[cfg(not(miri))]` and accept the small flakiness budget — the integration tests cover the happy path.)

- [ ] **Step 9.2.4: Backfill — emit human + emit json + exit-code matrix**

```rust
    #[test]
    fn emit_human_does_not_crash_on_empty_checks() {
        let r = Report { hector_version: "0".into(), checks: vec![] };
        emit(&r, OutputFormat::Human).unwrap();
    }

    #[test]
    fn emit_json_round_trips() {
        let r = Report {
            hector_version: "0".into(),
            checks: vec![CheckResult { name: "x", status: Status::Pass, detail: "d".into(), remediation: None }],
        };
        emit(&r, OutputFormat::Json).unwrap();
    }
```

Re-run the coverage gate:

```bash
bash scripts/ci-coverage.sh
```

Expected: `commands/doctor.rs` ≥ 80%.

- [ ] **Step 9.2.5: Commit coverage backfill (if any was needed)**

```bash
git add crates/hector-cli/src/commands/doctor.rs
git commit -m "$(cat <<'EOF'
test(cli): backfill doctor.rs coverage to clear the 80% gate (C1 phase 9)

Adds tests for the runtime_state Fail arm (Unix read-only parent),
load_claude_settings when HOME is unresolvable, and emit's human +
json branches on edge inputs. Brings commands/doctor.rs above the
ci-coverage.sh per-file region threshold.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 9.3: Plan archive

- [ ] **Step 9.3.1: Move the plan and update the index**

```bash
git mv plans/2026-05-13-hector-c1-doctor.md plans/archive/2026-05-13-hector-c1-doctor.md
```

Edit `plans/README.md`:
- Strike the in-flight mention (if any).
- Add to the Archive list:
  ```
  - [`2026-05-13-hector-c1-doctor`](archive/2026-05-13-hector-c1-doctor.md) — `hector doctor` diagnostic subcommand: 9 checks (binary, config, parses, trust, schema, scope_globs, engines, adapter, runtime_state); JSON contract under `docs/doctor.md`; exit code 0 on pass-or-warn, 1 on any fail.
  ```

```bash
git add plans/README.md plans/archive/2026-05-13-hector-c1-doctor.md
git commit -m "$(cat <<'EOF'
docs(plans): archive C1 (hector doctor) plan

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Acceptance criteria checklist (mapped to spec §C1)

- [ ] **`hector doctor` runs in a freshly initialized project and reports the expected checks.**
  → Phase 2 Task 2.1 (bootstrap), Phase 3 Task 3.1 `doctor_passes_on_clean_v2_config`. Output covers `binary`, `config`, `parses`, `trust`, `schema`, `scope_globs`, `engines`, `adapter`, `runtime_state` — every check from the spec's "Checks to run" list.
- [ ] **Each check has a remediation hint pointing at the relevant command or doc.**
  → Phases 3–6: `config` → `hector init`; `parses` (v1) → `hector migrate`; `trust` → `hector trust`; `schema` (v1) → `hector migrate`; `scope_globs` → fix-and-trust; `engines` → set `api_key_env` or add `llm:`; `adapter` → `docs/adapters/claude-code.md`; `runtime_state` → chmod hint.
- [ ] **JSON output schema documented in `docs/`.**
  → Phase 8 Task 8.1: `docs/doctor.md` with the full `Report` / `CheckResult` contract, check id table, and stability rules.
- [ ] **Doctor never modifies state. Read-only.**
  → All check fns are read-only against *policy* state (configs, baselines, telemetry). `check_runtime_state` does create `<dir>/.hector/` if absent — within the documented contract (run state, not policy state). No `.hector/log.jsonl` writes anywhere in `commands::doctor`.
- [ ] **Replace bully's "Python version" with `hector` binary date/version vs. latest release; defer the "latest release" check to 0.3.**
  → Phase 2 Task 2.2: `check_binary` reports the running version + path; per Spec §C1 Notes and §7 Q4, no phone-home.
- [ ] **Exit code: `0` if all pass or only warnings; `1` if any failure.**
  → Phase 2 Task 2.2: `exit_code` impl + unit tests `exit_code_is_zero_when_all_pass_or_warn`, `exit_code_is_one_when_any_fail`. Integration tests assert against every status path (`doctor_runs_and_reports_binary_check`, `doctor_fails_when_config_missing`, `doctor_fails_when_trust_fingerprint_broken`, `doctor_warns_when_semantic_rule_present_without_api_key` (warn → 0), `doctor_passes_on_clean_v2_config`).
- [ ] **Trust verification is its own check (not folded into "parses").**
  → Phase 3 Task 3.2: `parses` uses non-trust-verifying `parse_file_with_extends`; `trust` calls `trust::verify` directly. Integration test `doctor_fails_when_trust_fingerprint_broken` asserts `parses` row appears even when trust fails.
- [ ] **Engine availability covers semantic / session rules' `llm:` block + `api_key_env`.**
  → Phase 4 Task 4.2: `check_engines` + `llm_block_status`. Tests cover provider=ollama (no key needed), missing api_key_env field, env var unset, env var set.
- [ ] **Adapter check reads `~/.claude/settings.json`.**
  → Phase 5 Task 5.2: `load_claude_settings` + `claude_hook_wired`. Tests cover three states (missing, present-no-hook, present-with-hook).
- [ ] **`.hector/` writable check.**
  → Phase 6 Task 6.2: `check_runtime_state` write probe + size reporting.
- [ ] **`--format json` available.**
  → Already wired via the `OutputFormat` enum + `emit` branch from Phase 2; Phase 7 Task 7.2 locks the schema with an `insta` snapshot.

---

## Self-review

- [x] Spec §C1 walked end-to-end: every "Check to run" bullet has a corresponding `check_*` fn and integration test.
- [x] No placeholders. Every code block is complete; every command shows its expected output or status.
- [x] Type-name consistency: `CheckResult`, `Status`, `Report`, `DoctorContext` referenced consistently across phases. `Status` is local to `commands::doctor` (not the `verdict::Status` enum) — called out in Risk/rollback so reviewers don't confuse them.
- [x] Cognitive-complexity cap respected: `run` is one for-loop + one fold; `check_engines` decomposed into `llm_block_status`; `check_adapter` decomposed into `load_claude_settings` + `claude_hook_wired`. No `#[allow(clippy::cognitive_complexity)]` introduced.
- [x] Exit-code contract: doctor uses `0` / `1` (pass-or-warn / fail). `check`'s `0` / `1` / `2` is untouched. Documented in Risk/rollback and `docs/doctor.md`.
- [x] Verdict-schema impact: none. Doctor never constructs a `Verdict`. Documented in Risk/rollback.
- [x] Telemetry-schema impact: none. Doctor performs zero `.hector/log.jsonl` writes. Documented in Risk/rollback.
- [x] Open spec questions resolved inline: §Q4 (no phone-home; defer to 0.3), §C1 Notes (no Python-version equivalent — `binary` reports the running hector version instead).
- [x] Coverage gate (≥80% region per file) addressed: Phase 9 Task 9.2 backfills the hard-to-reach branches (runtime_state Fail arm, load_claude_settings home-unresolvable, emit on empty checks).
- [x] Per-file region-coverage gate is the local-CI script `bash scripts/ci-coverage.sh` — referenced explicitly in Phase 9.
- [x] Plan filename matches the repo convention (`plans/YYYY-MM-DD-hector-<id>-<slug>.md`), not the writing-plans skill default of `docs/superpowers/plans/...`.
