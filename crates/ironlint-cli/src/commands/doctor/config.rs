use std::path::Path;

use super::{CheckResult, DoctorContext, Status};

pub(super) fn check_config_present(ctx: &DoctorContext) -> CheckResult {
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

pub(super) fn check_config_parses(ctx: &DoctorContext) -> CheckResult {
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
pub(super) fn check_script_paths(ctx: &DoctorContext) -> CheckResult {
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
pub(super) fn check_run_path(dir: &Path, check_id: &str, run: &str) -> Option<String> {
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
