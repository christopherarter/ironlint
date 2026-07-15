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
use config::{check_config_parses, check_config_present, check_script_paths};

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
mod tests;
