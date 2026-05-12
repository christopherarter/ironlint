use crate::cli::OutputFormat;
use anyhow::{anyhow, Result};
use hector_core::runner::{CheckInput, HectorEngine};
use hector_core::verdict::{Status, Verdict};
use std::path::{Path, PathBuf};

pub fn run(
    file: Option<PathBuf>,
    diff: Option<PathBuf>,
    session: bool,
    format: OutputFormat,
    config: &Path,
) -> Result<i32> {
    let engine = match HectorEngine::load(config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            return Ok(1);
        }
    };

    if session {
        let dir = config.parent().unwrap_or(std::path::Path::new("."));
        let state_path = dir.join(".hector/session.json");
        let state = hector_core::session_state::SessionState::load(&state_path)?;
        let verdict = engine.check_session(&state)?;
        emit(&verdict, format)?;
        hector_core::session_state::SessionState::clear(&state_path)?;
        return Ok(exit_code(&verdict));
    }

    let input = match (file, diff) {
        (Some(f), None) => {
            let content = std::fs::read_to_string(&f)?;
            CheckInput::File { path: f, content }
        }
        (None, Some(d)) => {
            let unified_diff = std::fs::read_to_string(&d)?;
            let path = first_file_in_diff(&unified_diff)
                .ok_or_else(|| anyhow!("could not infer file from diff"))?;
            CheckInput::Diff {
                file: path,
                unified_diff,
            }
        }
        _ => {
            eprintln!("ERROR: provide exactly one of --file or --diff");
            return Ok(1);
        }
    };

    let verdict = engine.check(input)?;
    emit(&verdict, format)?;
    Ok(exit_code(&verdict))
}

fn exit_code(v: &Verdict) -> i32 {
    match v.status {
        Status::Pass | Status::Warn => 0,
        Status::Block => 2,
    }
}

fn emit(v: &Verdict, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(v)?);
        }
        OutputFormat::Human => {
            for vio in &v.violations {
                eprintln!(
                    "{}: [{}] {}{}",
                    vio.severity_human(),
                    vio.rule_id,
                    vio.file,
                    vio.line.map(|l| format!(":{l}")).unwrap_or_default()
                );
                eprintln!("  {}", vio.message);
            }
            println!(
                "{}",
                match v.status {
                    Status::Pass => "pass",
                    Status::Warn => "warn",
                    Status::Block => "block",
                }
            );
        }
    }
    Ok(())
}

fn first_file_in_diff(diff: &str) -> Option<PathBuf> {
    diff.lines()
        .find_map(|l| l.strip_prefix("+++ b/").map(PathBuf::from))
}

trait SeverityHuman {
    fn severity_human(&self) -> &'static str;
}

impl SeverityHuman for hector_core::verdict::Violation {
    fn severity_human(&self) -> &'static str {
        match self.severity {
            hector_core::verdict::Severity::Error => "error",
            hector_core::verdict::Severity::Warning => "warn",
        }
    }
}
