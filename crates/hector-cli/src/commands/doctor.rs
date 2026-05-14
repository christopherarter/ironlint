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
    let _ = &ctx; // ctx unused until phase 3 wires the config-based checks.
    let checks: Vec<CheckResult> = vec![check_binary()];
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
