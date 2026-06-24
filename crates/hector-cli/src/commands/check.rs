use crate::cli::OutputFormat;
use anyhow::{Context, Result};
use hector_core::runner::{CheckInput, CheckOptions, ExplainOutcome, GateExplain, HectorEngine};
use hector_core::verdict::{Status, Verdict};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

#[allow(clippy::too_many_arguments)]
pub fn run(
    file: Option<PathBuf>,
    diff: Option<PathBuf>,
    content: Option<String>,
    format: OutputFormat,
    config: &Path,
    gates: Vec<String>,
    event: String,
    explain: bool,
    allow_external_paths: bool,
) -> Result<i32> {
    // Trust gate: refuse an unblessed or tampered config/gates before the engine
    // loads or any gate runs. This hashes the config + `.hector/gates/` now; a
    // write between here and gate execution is a known, accepted TOCTOU window
    // (the direnv-model limitation — no file locking in 0.3).
    if let Err(e) = hector_core::trust::ensure_trusted(config) {
        eprintln!("ERROR: {e:#}");
        return Ok(1);
    }
    let options = CheckOptions {
        gates: HashSet::new(),
        event,
        allow_external_paths,
    };
    let mut engine = match HectorEngine::builder().with_options(options).load(config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            return Ok(1);
        }
    };
    if let Some(code) = validate_gate_filter(&engine, &gates) {
        return Ok(code);
    }
    engine.set_gate_filter(gates.into_iter().collect());

    match (file, diff) {
        (Some(f), None) => run_file(&engine, f, content, format, explain),
        (None, Some(d)) => run_diff(&engine, &d, format, explain),
        _ => {
            eprintln!("ERROR: provide exactly one of --file or --diff");
            Ok(1)
        }
    }
}

fn run_file(
    engine: &HectorEngine,
    file: PathBuf,
    content: Option<String>,
    format: OutputFormat,
    explain: bool,
) -> Result<i32> {
    let content = match content {
        Some(c) => resolve_content_value(c)?,
        None => std::fs::read_to_string(&file)
            .with_context(|| format!("failed to read {}", file.display()))?,
    };
    let report = engine.check_with_explain(CheckInput::File {
        path: file,
        content,
    })?;
    if explain {
        print_explain(&report.explain);
    }
    emit(&report.verdict, format)?;
    Ok(exit_code(&report.verdict))
}

/// Check every non-deleted changed file in a unified diff. Gates read each
/// file's current on-disk content (gates don't consume diffs).
fn run_diff(
    engine: &HectorEngine,
    diff: &Path,
    format: OutputFormat,
    explain: bool,
) -> Result<i32> {
    let unified = std::fs::read_to_string(diff)?;
    let changed = hector_core::diff::parser::parse_unified(&unified)?;
    if changed.is_empty() {
        eprintln!("ERROR: no changed files in diff");
        return Ok(1);
    }
    let targets: Vec<_> = changed
        .iter()
        .filter(|f| f.op != hector_core::diff::ChangeOp::Deleted)
        .collect();
    let mut blocks = Vec::new();
    let mut errors = Vec::new();
    let mut passed = Vec::new();
    let mut explains: Vec<GateExplain> = Vec::new();
    let mut elapsed = 0u64;
    for f in targets {
        let content = std::fs::read_to_string(&f.path).unwrap_or_default();
        let r = engine.check_with_explain(CheckInput::File {
            path: f.path.clone(),
            content,
        })?;
        elapsed = elapsed.saturating_add(r.verdict.elapsed_ms);
        blocks.extend(r.verdict.blocks);
        errors.extend(r.verdict.errors);
        passed.extend(r.verdict.passed);
        explains.extend(r.explain);
    }
    let verdict = Verdict::from_outcomes(blocks, errors, passed, elapsed);
    if explain {
        print_explain(&explains);
    }
    emit(&verdict, format)?;
    Ok(exit_code(&verdict))
}

fn validate_gate_filter(engine: &HectorEngine, gates: &[String]) -> Option<i32> {
    if gates.is_empty() {
        return None;
    }
    let known: HashSet<&str> = engine.gate_ids().collect();
    let unknown: Vec<&str> = gates
        .iter()
        .map(|s| s.as_str())
        .filter(|id| !known.contains(id))
        .collect();
    if unknown.is_empty() {
        None
    } else {
        eprintln!("ERROR: unknown gate id(s): {}", unknown.join(", "));
        Some(1)
    }
}

fn print_explain(rows: &[GateExplain]) {
    for row in rows {
        let outcome = match &row.outcome {
            ExplainOutcome::Fire => "fire".to_string(),
            ExplainOutcome::Pass => "pass".to_string(),
            ExplainOutcome::Skipped { reason } => format!("skipped {reason}"),
        };
        eprintln!("{} {}", row.gate_id, outcome);
    }
}

fn exit_code(v: &Verdict) -> i32 {
    match v.status {
        Status::Block => 2,
        Status::InternalError => 3,
        _ => 0,
    }
}

fn emit(v: &Verdict, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(v)?),
        OutputFormat::Human => {
            for b in &v.blocks {
                eprintln!("block: [{}] {}", b.gate, b.file);
                eprintln!("  {}", b.message);
            }
            for e in &v.errors {
                eprintln!("error: [{}] {} ({})", e.gate, e.file, e.reason);
            }
            println!(
                "{}",
                match v.status {
                    Status::Pass => "pass",
                    Status::Block => "block",
                    Status::InternalError => "internal_error",
                    _ => "unknown",
                }
            );
        }
    }
    Ok(())
}

fn resolve_content_value(value: String) -> Result<String> {
    if value == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("failed to read --content from stdin (expected UTF-8)")?;
        Ok(buf)
    } else {
        Ok(value)
    }
}
