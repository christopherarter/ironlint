//! `hector doctor` diagnostic subcommand.
//!
//! Read-only. Walks a fixed list of gate-model static checks and prints a
//! checklist (human) or JSON report. Exit code: 0 on all-pass-or-warn, 1 on
//! any fail.
//!
//! Checks kept for the gate model:
//!   1. binary — hector binary + version (always pass once we're running)
//!   2. config  — `.hector.yml` exists
//!   3. parses  — config parses (extends resolved)
//!   4. check_scripts — each check whose `run` names a single-token path that
//!      starts with `.hector/` exists and is executable
//!   5. adapters — one row per supported harness that is detected on this
//!      machine or has hector installed: pass when installed+registered, fail
//!      when registered-but-broken (hook artifact missing), warn otherwise
//!
//! Dropped from the old model: trust fingerprint, schema_version probe,
//! scope_globs (Rule-based), engine/EngineKind availability, capability
//! sandbox row, baseline/runtime_state.

use crate::cli::OutputFormat;
use anyhow::Result;
use hector_core::adapter::{all_harnesses, status, AdapterEnv, HarnessStatus, Scope};
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
    pub hector_version: String,
    pub checks: Vec<CheckResult>,
}

struct DoctorContext {
    dir: PathBuf,
    config_path: PathBuf,
}

pub fn run(dir: &Path, format: OutputFormat) -> Result<i32> {
    let ctx = DoctorContext {
        dir: dir.to_path_buf(),
        config_path: dir.join(".hector.yml"),
    };
    let mut checks: Vec<CheckResult> = vec![
        check_binary(),
        check_config_present(&ctx),
        check_config_parses(&ctx),
        check_script_paths(&ctx),
    ];
    if let Ok(env) = AdapterEnv::from_process(dir.to_path_buf()) {
        checks.extend(check_adapters(&env));
    }
    let report = Report {
        hector_version: env!("CARGO_PKG_VERSION").to_string(),
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
/// `.hector/`, check that the path exists and is executable. Inline commands
/// (e.g. `grep -q TODO && exit 2`) are skipped — detection: `run` contains a
/// space or doesn't look like a file path.
fn check_script_paths(ctx: &DoctorContext) -> CheckResult {
    let cfg = match hector_core::config::parse_file_with_extends(&ctx.config_path) {
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
                "ensure check scripts exist under .hector/gates/ and are executable (chmod +x)"
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
    // Only check paths that look like they're under .hector/
    if !run.starts_with(".hector/") {
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
            Some(format!("re-run `hector init --harness {}`", s.harness)),
        )
    } else if !s.installed {
        (
            Status::Warn,
            "harness detected; hector hook not installed".to_string(),
            Some(format!("run `hector init --harness {}`", s.harness)),
        )
    } else if !s.registered {
        (
            Status::Warn,
            "hook artifact present but not registered in settings".to_string(),
            Some(format!("re-run `hector init --harness {}`", s.harness)),
        )
    } else if s.intact == Some(false) {
        (
            Status::Warn,
            "hook artifact modified since install".to_string(),
            Some("re-run `hector init` to restore".to_string()),
        )
    } else if s.current == Some(false) {
        (
            Status::Warn,
            "hook artifact outdated".to_string(),
            Some("re-run `hector init` to update".to_string()),
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

/// Per-harness adapter checks. Uses Local scope because `hector init` defaults
/// to a project-local install and `doctor` runs in a project; a status() error
/// for a harness is skipped rather than failing the whole report.
fn check_adapters(env: &AdapterEnv) -> Vec<CheckResult> {
    all_harnesses()
        .iter()
        .filter_map(|h| status(h, env, Scope::Local).ok())
        .filter_map(|s| adapter_check(&s))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_is_zero_when_all_pass_or_warn() {
        let report = Report {
            hector_version: "0".into(),
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
            hector_version: "0".into(),
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
            config_path: dir.join(".hector.yml"),
        }
    }

    #[test]
    fn config_present_pass_when_file_exists() {
        let d = tempdir().unwrap();
        fs::write(d.path().join(".hector.yml"), "checks: {}\n").unwrap();
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
    fn parses_pass_on_valid_checks_config() {
        let d = tempdir().unwrap();
        fs::write(
            d.path().join(".hector.yml"),
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
            d.path().join(".hector.yml"),
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
    fn check_run_path_skips_non_hector_paths() {
        assert!(check_run_path(Path::new("."), "g", "scripts/check.sh").is_none());
    }

    #[test]
    fn check_run_path_fails_missing_script() {
        let d = tempdir().unwrap();
        let result = check_run_path(d.path(), "g", ".hector/gates/missing.sh");
        assert!(result.is_some());
        assert!(result.unwrap().contains("not found"));
    }

    fn adapter_env(tmp: &std::path::Path) -> hector_core::adapter::AdapterEnv {
        hector_core::adapter::AdapterEnv {
            home: tmp.to_path_buf(),
            config_home: tmp.join(".config"),
            project_root: tmp.join("proj"),
        }
    }

    #[test]
    fn check_adapters_reports_installed_reasonix_as_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let env = adapter_env(tmp.path());
        let h = hector_core::adapter::all_harnesses()
            .into_iter()
            .find(|h| h.name == "reasonix")
            .unwrap();
        hector_core::adapter::install(&h, &env, hector_core::adapter::Scope::Global, false)
            .unwrap();
        let checks = check_adapters(&env);
        let r = checks
            .iter()
            .find(|c| c.name == "reasonix")
            .expect("reasonix reported");
        assert_eq!(r.status, Status::Pass);
        assert!(r.detail.contains("installed"));
    }

    #[test]
    fn check_adapters_reports_broken_adapter_as_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let env = adapter_env(tmp.path());
        let h = hector_core::adapter::all_harnesses()
            .into_iter()
            .find(|h| h.name == "reasonix")
            .unwrap();
        hector_core::adapter::install(&h, &env, hector_core::adapter::Scope::Global, false)
            .unwrap();
        // Delete the materialized artifact dir but leave the settings entry → broken.
        std::fs::remove_dir_all(env.config_home.join("hector/adapters/reasonix")).unwrap();
        let checks = check_adapters(&env);
        let r = checks
            .iter()
            .find(|c| c.name == "reasonix")
            .expect("reasonix reported");
        assert_eq!(r.status, Status::Fail);
    }

    fn harness_status(detected: bool, installed: bool, registered: bool) -> HarnessStatus {
        HarnessStatus {
            harness: "reasonix",
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
        let s = hector_core::adapter::HarnessStatus {
            harness: "reasonix",
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
            .contains("hector init --harness reasonix"));
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
}
