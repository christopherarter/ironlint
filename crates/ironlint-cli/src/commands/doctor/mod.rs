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
use serde::Serialize;
use std::path::PathBuf;

mod adapters;
mod config;

use adapters::adapter_section;
#[cfg(test)]
use adapters::{
    adapter_check, build_hook_deps_result, check_adapters, hook_deps_row, hook_deps_verdict,
    hooks_row, json_hook_adapter_wired,
};
#[cfg(test)]
use config::check_run_path;
use config::{check_config_parses, check_config_present, check_script_paths};
#[cfg(test)]
use ironlint_core::adapter::HarnessStatus;
#[cfg(test)]
use std::path::Path;

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

pub fn run(dir: &std::path::Path, format: OutputFormat) -> Result<i32> {
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
            detail: "config/checks not trusted".into(),
            remediation: Some("run `ironlint trust`".into()),
        },
    }
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
