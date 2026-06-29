use crate::cli::OutputFormat;
use anyhow::{Context, Result};
use hector_core::runner::{CheckExplain, CheckInput, CheckOptions, ExplainOutcome, HectorEngine};
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
    checks: Vec<String>,
    event: String,
    explain: bool,
    allow_external_paths: bool,
    force: bool,
) -> Result<i32> {
    if force && checks.is_empty() {
        eprintln!("ERROR: --force requires at least one --check <id>");
        return Ok(1);
    }
    // Trust gate: refuse an unblessed or tampered config/checks before the engine
    // loads or any check runs. This hashes the config + `.hector/gates/` now; a
    // write between here and check execution is a known, accepted TOCTOU window
    // (the direnv-model limitation — no file locking in 0.4).
    if let Err(e) = hector_core::trust::ensure_trusted(config) {
        eprintln!("ERROR: {e:#}");
        return Ok(1);
    }
    let options = CheckOptions {
        checks: HashSet::new(),
        event,
        allow_external_paths,
        force,
    };
    let mut engine = match HectorEngine::builder().with_options(options).load(config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            return Ok(1);
        }
    };
    if let Some(code) = validate_check_filter(&engine, &checks) {
        return Ok(code);
    }
    engine.set_check_filter(checks.into_iter().collect());

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
    let report = match engine.check_with_explain(CheckInput::File {
        path: file,
        content,
    }) {
        Ok(r) => r,
        Err(e) => {
            // e.g. an external path resolving outside config_dir: an
            // argument/config error, so exit 1 (mirrors the load-error path).
            eprintln!("ERROR: {e:#}");
            return Ok(1);
        }
    };
    if explain {
        print_explain(&report.explain);
    }
    emit(&report.verdict, format)?;
    Ok(exit_code(&report.verdict))
}

/// Check every non-deleted changed file in a unified diff. Checks read each
/// file's current on-disk content (checks don't consume diffs).
///
/// When `event == "pre-commit"` the engine's `check_set` is called once over
/// the full set of changed paths (run-once semantics). For all other events
/// the per-file loop runs each file through `check_with_explain` individually.
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
    let non_deleted: Vec<_> = changed
        .iter()
        .filter(|f| f.op != hector_core::diff::ChangeOp::Deleted)
        .collect();

    // Pre-commit: run each check once over the entire changed set.
    if engine.event() == "pre-commit" {
        let paths: Vec<PathBuf> = non_deleted.iter().map(|f| f.path.clone()).collect();
        let verdict = engine.check_set(&paths)?;
        emit(&verdict, format)?;
        return Ok(exit_code(&verdict));
    }

    // Write (and any future per-file event): loop once per changed file.
    let mut blocks = Vec::new();
    let mut errors = Vec::new();
    let mut passed = Vec::new();
    let mut explains: Vec<CheckExplain> = Vec::new();
    let mut elapsed = 0u64;
    for f in non_deleted {
        // A changed file we can't read (deleted between diff-gen and check,
        // permissions, non-UTF-8 bytes) is a hard error: fabricating empty
        // content would run every check against "" and let a real violation
        // pass vacuously. Surface it as exit 1, never a silent empty pass.
        let content = match std::fs::read_to_string(&f.path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "ERROR: failed to read changed file {}: {e}",
                    f.path.display()
                );
                return Ok(1);
            }
        };
        let r = match engine.check_with_explain(CheckInput::File {
            path: f.path.clone(),
            content,
        }) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("ERROR: {e:#}");
                return Ok(1);
            }
        };
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

fn validate_check_filter(engine: &HectorEngine, checks: &[String]) -> Option<i32> {
    if checks.is_empty() {
        return None;
    }
    let known: HashSet<&str> = engine.check_ids().collect();
    let unknown: Vec<&str> = checks
        .iter()
        .map(|s| s.as_str())
        .filter(|id| !known.contains(id))
        .collect();
    if unknown.is_empty() {
        None
    } else {
        eprintln!("ERROR: unknown check id(s): {}", unknown.join(", "));
        Some(1)
    }
}

fn print_explain(rows: &[CheckExplain]) {
    for row in rows {
        let outcome = match &row.outcome {
            ExplainOutcome::Fire => "fire".to_string(),
            ExplainOutcome::Pass => "pass".to_string(),
            ExplainOutcome::Skipped { reason } => format!("skipped {reason}"),
        };
        eprintln!("{} {}", row.check_id, outcome);
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
                eprintln!("block: [{}] {}", b.check, b.file.as_deref().unwrap_or(""));
                eprintln!("  {}", b.message);
            }
            for e in &v.errors {
                eprintln!(
                    "error: [{}] {} ({})",
                    e.check,
                    e.file.as_deref().unwrap_or(""),
                    e.reason
                );
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
