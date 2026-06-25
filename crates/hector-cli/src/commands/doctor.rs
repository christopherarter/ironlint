//! `hector doctor` diagnostic subcommand.
//!
//! Read-only. Walks a fixed list of gate-model static checks and prints a
//! checklist (human) or JSON report. Exit code: 0 on all-pass-or-warn, 1 on
//! any fail.
//!
//! Checks kept for the gate model:
//!   1. binary — hector binary + version (always pass once we're running)
//!   2. adapter — Claude Code PostToolUse hook wired to hector
//!   3. config  — `.hector.yml` exists
//!   4. parses  — config parses (extends resolved)
//!   5. gate_scripts — each gate whose `run` names a single-token path that
//!      starts with `.hector/` exists and is executable
//!
//! Dropped from the old model: trust fingerprint, schema_version probe,
//! scope_globs (Rule-based), engine/EngineKind availability, capability
//! sandbox row, baseline/runtime_state.

use crate::cli::OutputFormat;
use anyhow::Result;
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
    let checks: Vec<CheckResult> = vec![
        check_binary(),
        check_adapter(),
        check_config_present(&ctx),
        check_config_parses(&ctx),
        check_gate_scripts(&ctx),
    ];
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
            detail: format!("config parses ({} gate(s))", cfg.gates.len()),
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

/// For each gate whose `run` is a single token (no spaces) that starts with
/// `.hector/`, check that the path exists and is executable. Inline commands
/// (e.g. `grep -q TODO && exit 2`) are skipped — detection: `run` contains a
/// space or doesn't look like a file path.
fn check_gate_scripts(ctx: &DoctorContext) -> CheckResult {
    let cfg = match hector_core::config::parse_file_with_extends(&ctx.config_path) {
        Ok(c) => c,
        Err(_) => {
            return CheckResult {
                name: "gate_scripts",
                status: Status::Warn,
                detail: "skipped (config does not parse)".into(),
                remediation: None,
            };
        }
    };
    let mut bad: Vec<String> = Vec::new();
    for (id, gate) in &cfg.gates {
        if let Some(issue) = check_run_path(&ctx.dir, id, &gate.run) {
            bad.push(issue);
        }
    }
    if bad.is_empty() {
        CheckResult {
            name: "gate_scripts",
            status: Status::Pass,
            detail: format!("{} gate(s) checked", cfg.gates.len()),
            remediation: None,
        }
    } else {
        CheckResult {
            name: "gate_scripts",
            status: Status::Fail,
            detail: format!("missing/non-executable gate script(s): {}", bad.join("; ")),
            remediation: Some(
                "ensure gate scripts exist under .hector/gates/ and are executable (chmod +x)"
                    .into(),
            ),
        }
    }
}

/// Returns `Some(problem description)` if `run` looks like a script path that
/// is missing or not executable; `None` if the command is inline or the script
/// is fine.
fn check_run_path(dir: &Path, gate_id: &str, run: &str) -> Option<String> {
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
        return Some(format!("{gate_id}: {run} not found"));
    }
    // Check executable bit (Unix only; on Windows always passes).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&script) {
            if meta.permissions().mode() & 0o111 == 0 {
                return Some(format!("{gate_id}: {run} not executable"));
            }
        }
    }
    None
}

/// Locate `~/.claude/settings.json`.
fn load_claude_settings() -> Option<(PathBuf, serde_json::Value)> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let path = PathBuf::from(home).join(".claude").join("settings.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    let value = serde_json::from_str(&raw).ok()?;
    Some((path, value))
}

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

fn check_adapter() -> CheckResult {
    let Some((path, settings)) = load_claude_settings() else {
        return CheckResult {
            name: "adapter",
            status: Status::Warn,
            detail: "Claude Code adapter not detected (~/.claude/settings.json missing)".into(),
            remediation: Some(
                "if you use Claude Code, install the adapter — see docs/adapters/claude-code.md"
                    .into(),
            ),
        };
    };
    if claude_hook_wired(&settings) {
        CheckResult {
            name: "adapter",
            status: Status::Pass,
            detail: format!(
                "Claude Code PostToolUse hook references hector ({})",
                path.display()
            ),
            remediation: None,
        }
    } else {
        CheckResult {
            name: "adapter",
            status: Status::Warn,
            detail: format!(
                "{} present but no PostToolUse hook references hector",
                path.display()
            ),
            remediation: Some(
                "install the adapter or add a PostToolUse entry calling hector — see docs/adapters/claude-code.md".into(),
            ),
        }
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
        fs::write(d.path().join(".hector.yml"), "gates: {}\n").unwrap();
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
    fn parses_pass_on_valid_gates_config() {
        let d = tempdir().unwrap();
        fs::write(
            d.path().join(".hector.yml"),
            "gates:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
        )
        .unwrap();
        let r = check_config_parses(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
        assert!(r.detail.contains("1 gate(s)"));
    }

    #[test]
    fn hook_wired_finds_hector_command() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"hector check"}]}]}}"#,
        )
        .unwrap();
        assert!(claude_hook_wired(&v));
    }

    #[test]
    fn hook_wired_finds_adapter_hook_sh() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"$ROOT/hooks/hook.sh post"}]}]}}"#,
        )
        .unwrap();
        assert!(claude_hook_wired(&v));
    }

    #[test]
    fn hook_wired_rejects_unrelated_command() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
        )
        .unwrap();
        assert!(!claude_hook_wired(&v));
    }

    #[test]
    fn hook_wired_rejects_empty_object() {
        let v: serde_json::Value = serde_json::from_str(r"{}").unwrap();
        assert!(!claude_hook_wired(&v));
    }

    #[test]
    fn gate_scripts_warn_when_config_missing() {
        let d = tempdir().unwrap();
        let r = check_gate_scripts(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Warn);
    }

    #[test]
    fn gate_scripts_pass_for_inline_commands() {
        let d = tempdir().unwrap();
        fs::write(
            d.path().join(".hector.yml"),
            "gates:\n  g:\n    files: \"*.rs\"\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
        )
        .unwrap();
        let r = check_gate_scripts(&ctx_with(d.path()));
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
}
