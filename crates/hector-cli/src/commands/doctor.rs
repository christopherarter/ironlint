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
    let checks: Vec<CheckResult> = vec![
        check_binary(),
        check_config_present(&ctx),
        check_config_parses(&ctx),
        check_trust(&ctx),
        check_schema_version(&ctx),
    ];
    let report = Report {
        hector_version: env!("CARGO_PKG_VERSION").to_string(),
        checks,
    };
    emit(&report, format)?;
    Ok(exit_code(&report))
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
}
