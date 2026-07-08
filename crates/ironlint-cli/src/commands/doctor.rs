//! `ironlint doctor` diagnostic subcommand.
//!
//! Read-only. Walks a fixed list of gate-model static checks and prints a
//! checklist (human) or JSON report. Exit code: 0 on all-pass-or-warn, 1 on
//! any fail.
//!
//! Checks kept for the gate model:
//!   1. binary — ironlint binary + version (always pass once we're running)
//!   2. config  — `.ironlint.yml` exists
//!   3. parses  — config parses (extends resolved)
//!   4. check_scripts — each check whose `run` names a single-token path that
//!      starts with `.ironlint/scripts/` exists and is executable
//!   5. trust — config + `.ironlint/scripts/` are blessed in the trust store
//!      (warn, not fail: doctor is read-only; trust is enforced only at the
//!      `check` layer)
//!   6. adapters — one row per supported harness that is detected on this
//!      machine or has ironlint installed: pass when installed+registered, fail
//!      when registered-but-broken (hook artifact missing), warn otherwise
//!   7. hooks — always-present summary row: warns when zero coding-agent hooks
//!      are wired (the most common first-run failure mode)
//!
//! Dropped from the old model: schema_version probe, scope_globs (Rule-based),
//! engine/EngineKind availability, capability sandbox row, baseline/runtime_state.

use crate::cli::OutputFormat;
use crate::commands::check;
use anyhow::Result;
use ironlint_core::adapter::{
    all_harnesses, status, AdapterEnv, HarnessKind, HarnessStatus, Scope,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub ironlint_version: String,
    pub checks: Vec<CheckResult>,
}

struct DoctorContext {
    dir: PathBuf,
    config_path: PathBuf,
}

pub fn run(dir: &Path, format: OutputFormat) -> Result<i32> {
    let ctx = DoctorContext {
        dir: dir.to_path_buf(),
        config_path: dir.join(".ironlint.yml"),
    };
    let mut checks: Vec<CheckResult> = vec![
        check_binary(),
        check_config_present(&ctx),
        check_config_parses(&ctx),
        check_script_paths(&ctx),
        shell_row(),
        trust_row(&ctx),
    ];
    checks.extend(adapter_section(dir));
    let report = Report {
        ironlint_version: env!("CARGO_PKG_VERSION").to_string(),
        checks,
    };
    emit(&report, format)?;
    Ok(exit_code(&report))
}

fn exit_code(report: &Report) -> i32 {
    i32::from(report.checks.iter().any(|c| c.status == Status::Fail))
}

fn emit(report: &Report, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(report)?);
        }
        OutputFormat::Human => {
            println!("ironlint doctor — version {}", report.ironlint_version);
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

fn check_binary() -> CheckResult {
    let path = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "<unknown>".into());
    CheckResult {
        name: "binary",
        status: Status::Pass,
        detail: format!("ironlint {} at {}", env!("CARGO_PKG_VERSION"), path),
        remediation: None,
    }
}

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
            remediation: Some("run `ironlint init` to scaffold a starter config".into()),
        }
    }
}

fn check_config_parses(ctx: &DoctorContext) -> CheckResult {
    if !ctx.config_path.exists() {
        return CheckResult {
            name: "parses",
            status: Status::Fail,
            detail: "config missing; nothing to parse".into(),
            remediation: Some("run `ironlint init` first".into()),
        };
    }
    match ironlint_core::config::parse_file_with_extends(&ctx.config_path) {
        Ok(cfg) => CheckResult {
            name: "parses",
            status: Status::Pass,
            detail: format!("config parses ({} check(s))", cfg.checks.len()),
            remediation: None,
        },
        Err(e) => CheckResult {
            name: "parses",
            status: Status::Fail,
            detail: format!("{e:#}"),
            remediation: Some("fix the YAML error above and re-run".into()),
        },
    }
}

/// For each check whose `run` is a single token (no spaces) that starts with
/// `.ironlint/scripts/`, check that the path exists and is executable. Inline commands
/// (e.g. `grep -q TODO && exit 2`) are skipped — detection: `run` contains a
/// space or doesn't look like a file path.
fn check_script_paths(ctx: &DoctorContext) -> CheckResult {
    let cfg = match ironlint_core::config::parse_file_with_extends(&ctx.config_path) {
        Ok(c) => c,
        Err(_) => {
            return CheckResult {
                name: "check_scripts",
                status: Status::Warn,
                detail: "skipped (config does not parse)".into(),
                remediation: None,
            };
        }
    };
    let mut bad: Vec<String> = Vec::new();
    for (id, check) in &cfg.checks {
        for step in check.effective_steps() {
            if let Some(issue) = check_run_path(&ctx.dir, id, &step.run) {
                bad.push(issue);
            }
        }
    }
    if bad.is_empty() {
        CheckResult {
            name: "check_scripts",
            status: Status::Pass,
            detail: format!("{} check(s) checked", cfg.checks.len()),
            remediation: None,
        }
    } else {
        CheckResult {
            name: "check_scripts",
            status: Status::Fail,
            detail: format!("missing/non-executable check script(s): {}", bad.join("; ")),
            remediation: Some(
                "ensure check scripts exist under .ironlint/scripts/ and are executable (chmod +x)"
                    .into(),
            ),
        }
    }
}

/// Returns `Some(problem description)` if `run` looks like a script path that
/// is missing or not executable; `None` if the command is inline or the script
/// is fine.
fn check_run_path(dir: &Path, check_id: &str, run: &str) -> Option<String> {
    // Inline command: contains a space → skip.
    if run.contains(' ') {
        return None;
    }
    // Only check paths that look like they're under .ironlint/scripts/
    if !run.starts_with(".ironlint/scripts/") {
        return None;
    }
    let script = dir.join(run);
    if !script.exists() {
        return Some(format!("{check_id}: {run} not found"));
    }
    // Check executable bit (Unix only; on Windows always passes).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&script) {
            if meta.permissions().mode() & 0o111 == 0 {
                return Some(format!("{check_id}: {run} not executable"));
            }
        }
    }
    None
}

/// Decide the (status, detail, remediation) triple for a harness that is
/// detected or installed. Split out of `adapter_check` so the if/else-if ladder
/// stays under the cognitive-complexity cap.
fn adapter_verdict(s: &HarnessStatus) -> (Status, String, Option<String>) {
    if s.registered && !s.installed {
        (
            Status::Fail,
            "registered in settings but hook artifact is missing (broken)".to_string(),
            Some(format!("re-run `ironlint init --harness {}`", s.harness)),
        )
    } else if !s.installed {
        (
            Status::Warn,
            "harness detected; ironlint hook not installed".to_string(),
            Some(format!("run `ironlint init --harness {}`", s.harness)),
        )
    } else if !s.registered {
        (
            Status::Warn,
            "hook artifact present but not registered in settings".to_string(),
            Some(format!("re-run `ironlint init --harness {}`", s.harness)),
        )
    } else if s.intact == Some(false) {
        (
            Status::Warn,
            "hook artifact modified since install".to_string(),
            Some("re-run `ironlint init` to restore".to_string()),
        )
    } else if s.current == Some(false) {
        (
            Status::Warn,
            "hook artifact outdated".to_string(),
            Some("re-run `ironlint init` to update".to_string()),
        )
    } else {
        (Status::Pass, "installed and registered".to_string(), None)
    }
}

/// Map one harness's status to a doctor CheckResult. Returns None for a
/// harness that is neither present nor installed (no signal worth a line).
fn adapter_check(s: &HarnessStatus) -> Option<CheckResult> {
    if !s.detected && !s.installed && !s.registered {
        return None;
    }
    let (status, detail, remediation) = adapter_verdict(s);
    Some(CheckResult {
        name: s.harness,
        status,
        detail,
        remediation,
    })
}

/// Shell availability row: `pass` when the POSIX shell the engine spawns
/// (`sh`) is on PATH, `fail` with a clear remediation when it is not. This is
/// the fail-loud guard for stock Windows, where `sh` is absent and every check
/// would otherwise fail to spawn (exit 3 → adapters fail open). Reuses the same
/// probe `check::run` gates on at startup (`check::shell_available`).
fn shell_row() -> CheckResult {
    if check::shell_available(check::POSIX_SHELL) {
        CheckResult {
            name: "shell",
            status: Status::Pass,
            detail: format!("`{}` found on PATH", check::POSIX_SHELL),
            remediation: None,
        }
    } else {
        CheckResult {
            name: "shell",
            status: Status::Fail,
            detail: format!("no POSIX shell (`{}`) on PATH", check::POSIX_SHELL),
            remediation: Some(
                "on Windows, run IronLint inside Git Bash or WSL; see docs/getting-started.md"
                    .into(),
            ),
        }
    }
}

/// Trust row: warn (not fail) when the config is not blessed. Doctor is
/// read-only — trust enforcement lives at the `check` layer (`commands/check.rs`
/// calls `trust::check_trust` and exits 4 on a mismatch, Task 3.2). A `warn`
/// here surfaces the gap without making a read-only command fail merely
/// because the config is unblessed.
fn trust_row(ctx: &DoctorContext) -> CheckResult {
    if !ctx.config_path.exists() {
        return CheckResult {
            name: "trust",
            status: Status::Warn,
            detail: "skipped (no config to trust)".into(),
            remediation: Some("run `ironlint init` to scaffold a config first".into()),
        };
    }
    match ironlint_core::trust::ensure_trusted(&ctx.config_path) {
        Ok(()) => CheckResult {
            name: "trust",
            status: Status::Pass,
            detail: "config is trusted".into(),
            remediation: None,
        },
        Err(_) => CheckResult {
            name: "trust",
            status: Status::Warn,
            detail: "config/gates not trusted".into(),
            remediation: Some("run `ironlint trust`".into()),
        },
    }
}

/// Always-present summary row over the per-harness adapter rows. Warns when
/// zero coding-agent hooks are wired — the most common first-run failure mode,
/// since the tool's entire effect happens through hooks. Only a `Pass` adapter
/// row (installed AND registered) counts as wired: a `Warn` row (detected but
/// ironlint not installed) or a `Fail` row (registered but broken) is present
/// but NOT wired, so a machine with, e.g., Claude Code installed and `ironlint
/// init` never run must still warn — not report a healthy install.
fn hooks_row(adapter_rows: &[CheckResult]) -> CheckResult {
    let wired = adapter_rows
        .iter()
        .filter(|r| r.status == Status::Pass)
        .count();
    if wired == 0 {
        CheckResult {
            name: "hooks",
            status: Status::Warn,
            detail: "no coding-agent hooks detected".into(),
            remediation: Some("run `ironlint init`".into()),
        }
    } else {
        CheckResult {
            name: "hooks",
            status: Status::Pass,
            detail: format!("{wired} harness(es) wired"),
            remediation: None,
        }
    }
}

/// Per-harness adapter checks. Uses Local scope because `ironlint init` defaults
/// to a project-local install and `doctor` runs in a project; a status() error
/// for a harness is skipped rather than failing the whole report.
fn check_adapters(env: &AdapterEnv) -> Vec<CheckResult> {
    all_harnesses()
        .iter()
        .filter_map(|h| status(h, env, Scope::Local).ok())
        .filter_map(|s| adapter_check(&s))
        .collect()
}

/// The adapter block of the report: per-harness rows, the `hook deps` row (only
/// when a JSON-hook adapter is wired), and the always-present `hooks` summary.
/// Ordering reads as: each adapter, then its runtime deps, then the summary.
/// When the adapter environment can't be resolved, no adapter is detectable,
/// so only the (warning) hooks summary over an empty set is emitted.
fn adapter_section(dir: &Path) -> Vec<CheckResult> {
    let Ok(env) = AdapterEnv::from_process(dir.to_path_buf()) else {
        return vec![hooks_row(&[])];
    };
    let adapter_rows = check_adapters(&env);
    let hooks = hooks_row(&adapter_rows);
    let mut section = adapter_rows;
    if let Some(deps) = hook_deps_row(&env) {
        section.push(deps);
    }
    section.push(hooks);
    section
}

/// True when at least one JSON-hook adapter (claude-code / codex) is installed
/// or registered on this machine. Those hooks shell out to `jq` and `python3`;
/// the TS-plugin harnesses (pi / opencode) don't, so they're excluded. A merely
/// *detected* harness (ironlint hook not wired in) doesn't count — the hook
/// can't fire, so its deps aren't yet relevant.
fn json_hook_adapter_wired(env: &AdapterEnv) -> bool {
    all_harnesses()
        .iter()
        .filter(|h| matches!(h.kind, HarnessKind::JsonHook(_)))
        .filter_map(|h| status(h, env, Scope::Local).ok())
        .any(|s| s.installed || s.registered)
}

/// Pure decision for the `hook deps` row, split from PATH probing so it's
/// unit-testable without depending on what's installed on the test machine.
/// - not wired → `None` (no JSON-hook adapter → jq/python3 irrelevant, no row)
/// - wired, both present → `Some(Pass, [])`
/// - wired, any missing → `Some(Fail, [missing names in a stable order])`
fn hook_deps_verdict(
    wired: bool,
    jq_present: bool,
    python3_present: bool,
) -> Option<(Status, Vec<&'static str>)> {
    if !wired {
        return None;
    }
    let mut missing: Vec<&'static str> = Vec::new();
    if !jq_present {
        missing.push("jq");
    }
    if !python3_present {
        missing.push("python3");
    }
    let status = if missing.is_empty() {
        Status::Pass
    } else {
        Status::Fail
    };
    Some((status, missing))
}

/// Render the pure [`hook_deps_verdict`] decision into a `CheckResult`. A Fail
/// names the missing binaries and spells out the consequence: a missing dep
/// makes the JSON-hook adapters fail OPEN, silently un-gating every edit.
fn build_hook_deps_result((status, missing): (Status, Vec<&'static str>)) -> CheckResult {
    if status == Status::Pass {
        return CheckResult {
            name: "hook deps",
            status: Status::Pass,
            detail: "`jq` and `python3` found on PATH".into(),
            remediation: None,
        };
    }
    let pronoun = if missing.len() > 1 { "them" } else { "it" };
    CheckResult {
        name: "hook deps",
        status: Status::Fail,
        detail: format!("missing on PATH: {}", missing.join(", ")),
        remediation: Some(format!(
            "install {} — the claude-code/codex hook needs {pronoun} or it fails open \
             (every edit is silently un-gated)",
            missing.join(" and "),
        )),
    }
}

/// The `hook deps` row: probe `jq`/`python3` on PATH, but only when a JSON-hook
/// adapter is actually wired (otherwise the deps are irrelevant → `None`).
fn hook_deps_row(env: &AdapterEnv) -> Option<CheckResult> {
    let verdict = hook_deps_verdict(
        json_hook_adapter_wired(env),
        check::binary_available("jq"),
        check::binary_available("python3"),
    )?;
    Some(build_hook_deps_result(verdict))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_is_zero_when_all_pass_or_warn() {
        let report = Report {
            ironlint_version: "0".into(),
            checks: vec![
                CheckResult {
                    name: "a",
                    status: Status::Pass,
                    detail: "".into(),
                    remediation: None,
                },
                CheckResult {
                    name: "b",
                    status: Status::Warn,
                    detail: "".into(),
                    remediation: None,
                },
            ],
        };
        assert_eq!(exit_code(&report), 0);
    }

    #[test]
    fn exit_code_is_one_when_any_fail() {
        let report = Report {
            ironlint_version: "0".into(),
            checks: vec![
                CheckResult {
                    name: "a",
                    status: Status::Pass,
                    detail: "".into(),
                    remediation: None,
                },
                CheckResult {
                    name: "b",
                    status: Status::Fail,
                    detail: "boom".into(),
                    remediation: Some("fix it".into()),
                },
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

    use std::fs;
    use tempfile::tempdir;

    fn ctx_with(dir: &std::path::Path) -> DoctorContext {
        DoctorContext {
            dir: dir.to_path_buf(),
            config_path: dir.join(".ironlint.yml"),
        }
    }

    #[test]
    fn config_present_pass_when_file_exists() {
        let d = tempdir().unwrap();
        fs::write(d.path().join(".ironlint.yml"), "checks: {}\n").unwrap();
        let r = check_config_present(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
    }

    #[test]
    fn config_present_fail_when_file_missing() {
        let d = tempdir().unwrap();
        let r = check_config_present(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Fail);
        assert!(r.remediation.unwrap().contains("ironlint init"));
    }

    #[test]
    fn parses_fail_when_config_missing() {
        let d = tempdir().unwrap();
        let r = check_config_parses(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Fail);
    }

    #[test]
    fn parses_pass_on_valid_checks_config() {
        let d = tempdir().unwrap();
        fs::write(
            d.path().join(".ironlint.yml"),
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
        )
        .unwrap();
        let r = check_config_parses(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
        assert!(r.detail.contains("1 check(s)"));
    }

    #[test]
    fn check_scripts_warn_when_config_missing() {
        let d = tempdir().unwrap();
        let r = check_script_paths(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Warn);
    }

    #[test]
    fn check_scripts_pass_for_inline_commands() {
        let d = tempdir().unwrap();
        fs::write(
            d.path().join(".ironlint.yml"),
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
        )
        .unwrap();
        let r = check_script_paths(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
    }

    #[test]
    fn check_run_path_skips_inline_commands() {
        assert!(check_run_path(Path::new("."), "g", "grep -q TODO").is_none());
    }

    #[test]
    fn check_run_path_skips_non_ironlint_paths() {
        assert!(check_run_path(Path::new("."), "g", "scripts/check.sh").is_none());
    }

    #[test]
    fn check_run_path_fails_missing_script() {
        let d = tempdir().unwrap();
        let result = check_run_path(d.path(), "g", ".ironlint/scripts/missing.sh");
        assert!(result.is_some());
        assert!(result.unwrap().contains("not found"));
    }

    #[test]
    fn check_run_path_skips_legacy_gates_path() {
        // After the rename, doctor only checks scripts under .ironlint/scripts/.
        // A legacy .ironlint/gates/ path is not the policy surface and is skipped
        // (returns None) rather than flagged as missing.
        assert!(check_run_path(Path::new("."), "g", ".ironlint/gates/missing.sh").is_none());
    }

    fn adapter_env(tmp: &std::path::Path) -> ironlint_core::adapter::AdapterEnv {
        ironlint_core::adapter::AdapterEnv {
            home: tmp.to_path_buf(),
            config_home: tmp.join(".config"),
            project_root: tmp.join("proj"),
        }
    }

    #[test]
    fn check_adapters_reports_installed_codex_as_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let env = adapter_env(tmp.path());
        let h = ironlint_core::adapter::all_harnesses()
            .into_iter()
            .find(|h| h.name == "codex")
            .unwrap();
        // check_adapters reads status at Scope::Local; codex (unlike the prior global-only harness)
        // has a real project-local settings file, so install must match scope.
        ironlint_core::adapter::install(&h, &env, ironlint_core::adapter::Scope::Local).unwrap();
        let checks = check_adapters(&env);
        let r = checks
            .iter()
            .find(|c| c.name == "codex")
            .expect("codex reported");
        assert_eq!(r.status, Status::Pass);
        assert!(r.detail.contains("installed"));
    }

    #[test]
    fn check_adapters_reports_broken_adapter_as_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let env = adapter_env(tmp.path());
        let h = ironlint_core::adapter::all_harnesses()
            .into_iter()
            .find(|h| h.name == "codex")
            .unwrap();
        // check_adapters reads status at Scope::Local; codex (unlike the prior global-only harness)
        // has a real project-local settings file, so install must match scope.
        ironlint_core::adapter::install(&h, &env, ironlint_core::adapter::Scope::Local).unwrap();
        // Delete the materialized artifact dir but leave the settings entry → broken.
        std::fs::remove_dir_all(env.config_home.join("ironlint/adapters/codex")).unwrap();
        let checks = check_adapters(&env);
        let r = checks
            .iter()
            .find(|c| c.name == "codex")
            .expect("codex reported");
        assert_eq!(r.status, Status::Fail);
    }

    fn harness_status(detected: bool, installed: bool, registered: bool) -> HarnessStatus {
        HarnessStatus {
            harness: "codex",
            detected,
            installed,
            registered,
            intact: Some(true),
            current: Some(true),
        }
    }

    #[test]
    fn adapter_check_skips_when_neither_detected_nor_installed() {
        let s = harness_status(false, false, false);
        assert!(adapter_check(&s).is_none());
    }

    #[test]
    fn adapter_check_reports_registered_but_absent_as_fail() {
        // registered in settings but artifact gone AND harness dir absent:
        // must still surface as a broken (Fail) row, not be skipped.
        let s = ironlint_core::adapter::HarnessStatus {
            harness: "codex",
            detected: false,
            installed: false,
            registered: true,
            intact: None,
            current: None,
        };
        let c = adapter_check(&s).expect("registered-but-absent must not be skipped");
        assert_eq!(c.status, Status::Fail);
    }

    #[test]
    fn adapter_check_warns_when_detected_but_not_installed() {
        let s = harness_status(true, false, false);
        let r = adapter_check(&s).expect("detected harness reported");
        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("not installed"));
        assert!(r
            .remediation
            .unwrap()
            .contains("ironlint init --harness codex"));
    }

    #[test]
    fn adapter_check_warns_when_installed_but_not_registered() {
        let s = harness_status(true, true, false);
        let r = adapter_check(&s).expect("installed harness reported");
        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("not registered"));
    }

    #[test]
    fn adapter_check_warns_when_artifact_modified() {
        let mut s = harness_status(true, true, true);
        s.intact = Some(false);
        let r = adapter_check(&s).expect("modified harness reported");
        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("modified"));
    }

    #[test]
    fn adapter_check_warns_when_artifact_outdated() {
        let mut s = harness_status(true, true, true);
        s.current = Some(false);
        let r = adapter_check(&s).expect("outdated harness reported");
        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("outdated"));
    }

    #[test]
    fn adapter_check_passes_when_installed_and_registered() {
        let s = harness_status(true, true, true);
        let r = adapter_check(&s).expect("healthy harness reported");
        assert_eq!(r.status, Status::Pass);
        assert!(r.remediation.is_none());
    }

    fn row(name: &'static str, status: Status) -> CheckResult {
        CheckResult {
            name,
            status,
            detail: String::new(),
            remediation: None,
        }
    }

    #[test]
    fn hooks_row_warns_when_no_adapter_rows() {
        let r = hooks_row(&[]);
        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("no coding-agent hooks"));
        assert!(r.remediation.unwrap().contains("ironlint init"));
    }

    #[test]
    fn hooks_row_warns_when_only_unwired_rows() {
        // A detected-but-not-installed harness surfaces as a Warn adapter row; it
        // is NOT wired, so the summary must warn, not report a healthy install.
        let rows = [row("codex", Status::Warn)];
        let r = hooks_row(&rows);
        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("no coding-agent hooks"));
    }

    #[test]
    fn hooks_row_warns_when_only_broken_rows() {
        // A registered-but-broken harness surfaces as a Fail adapter row; still
        // zero hooks are actually wired, so the summary must warn.
        let rows = [row("codex", Status::Fail)];
        let r = hooks_row(&rows);
        assert_eq!(r.status, Status::Warn);
    }

    #[test]
    fn hooks_row_counts_only_wired_pass_rows() {
        // One wired (Pass) harness + one unwired (Warn) harness → pass, but the
        // count reports only the wired one.
        let rows = [row("codex", Status::Pass), row("pi", Status::Warn)];
        let r = hooks_row(&rows);
        assert_eq!(r.status, Status::Pass);
        assert!(
            r.detail.contains("1 harness(es) wired"),
            "detail must count only wired rows: {}",
            r.detail
        );
    }

    // --- Task 5.23 Part 2: hook-dependency (jq/python3) probe -------------

    #[test]
    fn hook_deps_omitted_when_no_json_hook_adapter() {
        // No JSON-hook adapter wired → jq/python3 irrelevant → no row at all,
        // regardless of what happens to be on PATH.
        assert!(hook_deps_verdict(false, true, true).is_none());
        assert!(hook_deps_verdict(false, false, false).is_none());
    }

    #[test]
    fn hook_deps_pass_when_both_present() {
        let (status, missing) = hook_deps_verdict(true, true, true).expect("wired → a row");
        assert_eq!(status, Status::Pass);
        assert!(missing.is_empty(), "nothing missing: {missing:?}");
    }

    #[test]
    fn hook_deps_fail_names_missing_jq() {
        let (status, missing) = hook_deps_verdict(true, false, true).expect("wired → a row");
        assert_eq!(status, Status::Fail);
        assert_eq!(missing, vec!["jq"]);
    }

    #[test]
    fn hook_deps_fail_names_missing_python3() {
        let (status, missing) = hook_deps_verdict(true, true, false).expect("wired → a row");
        assert_eq!(status, Status::Fail);
        assert_eq!(missing, vec!["python3"]);
    }

    #[test]
    fn hook_deps_fail_names_both_when_both_missing() {
        let (status, missing) = hook_deps_verdict(true, false, false).expect("wired → a row");
        assert_eq!(status, Status::Fail);
        assert_eq!(missing, vec!["jq", "python3"]);
    }

    #[test]
    fn hook_deps_result_fail_has_remediation_naming_missing() {
        let r = build_hook_deps_result((Status::Fail, vec!["jq"]));
        assert_eq!(r.name, "hook deps");
        assert_eq!(r.status, Status::Fail);
        assert!(
            r.detail.contains("jq"),
            "detail names the missing dep: {}",
            r.detail
        );
        let rem = r.remediation.expect("fail row must remediate");
        assert!(
            rem.contains("jq"),
            "remediation names the missing dep: {rem}"
        );
        assert!(
            rem.contains("fails open"),
            "remediation must explain the fail-open consequence: {rem}"
        );
    }

    #[test]
    fn hook_deps_result_pass_has_no_remediation() {
        let r = build_hook_deps_result((Status::Pass, vec![]));
        assert_eq!(r.name, "hook deps");
        assert_eq!(r.status, Status::Pass);
        assert!(r.remediation.is_none());
    }

    #[test]
    fn json_hook_adapter_wired_false_on_clean_env() {
        let tmp = tempfile::tempdir().unwrap();
        let env = adapter_env(tmp.path());
        assert!(!json_hook_adapter_wired(&env));
    }

    #[test]
    fn json_hook_adapter_wired_true_when_codex_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let env = adapter_env(tmp.path());
        let h = ironlint_core::adapter::all_harnesses()
            .into_iter()
            .find(|h| h.name == "codex")
            .unwrap();
        ironlint_core::adapter::install(&h, &env, ironlint_core::adapter::Scope::Local).unwrap();
        assert!(json_hook_adapter_wired(&env));
    }

    #[test]
    fn hook_deps_row_none_when_no_json_hook_adapter() {
        // A clean env has no wired JSON-hook adapter → no deps row, even though
        // jq/python3 may be present on the machine running the test.
        let tmp = tempfile::tempdir().unwrap();
        let env = adapter_env(tmp.path());
        assert!(hook_deps_row(&env).is_none());
    }

    #[test]
    fn hook_deps_row_present_when_json_hook_adapter_wired() {
        // Installing a JSON-hook adapter makes the row fire. The Pass/Fail
        // status depends on whether the machine has jq/python3, but a row is
        // ALWAYS produced once wired — that Some/None split is machine-agnostic.
        let tmp = tempfile::tempdir().unwrap();
        let env = adapter_env(tmp.path());
        let h = ironlint_core::adapter::all_harnesses()
            .into_iter()
            .find(|h| h.name == "codex")
            .unwrap();
        ironlint_core::adapter::install(&h, &env, ironlint_core::adapter::Scope::Local).unwrap();
        let r = hook_deps_row(&env).expect("wired adapter → a deps row");
        assert_eq!(r.name, "hook deps");
    }
}
