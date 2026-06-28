//! `hector explain <file>` — read-only check applicability report.
//!
//! For each check in the resolved config (BTreeMap id order), reports whether
//! the check's file globs apply to the given path (`match`) or not (`skip`).
//! `human` (default) prints one line per check
//! `<check-id>  <match|skip>  files=<globs>  run=<run>`; `json` prints an array
//! of `{ check, status, files, run }` objects. No check logic is executed.
//! Errors go to stderr; exit 1.

use crate::cli::OutputFormat;
use anyhow::Result;
use hector_core::runner::HectorEngine;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
struct ExplainEntry<'a> {
    check: &'a str,
    status: &'static str,
    files: &'a [String],
    run: &'a str,
}

fn status_for(engine: &HectorEngine, check_id: &str, file: &Path) -> &'static str {
    if engine.gate_matches_path(check_id, file) {
        "match"
    } else {
        "skip"
    }
}

pub fn run(file: &Path, format: OutputFormat, config: &Path) -> Result<i32> {
    let engine = match HectorEngine::load(config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            return Ok(1);
        }
    };
    match format {
        OutputFormat::Human => print_human(&engine, file),
        OutputFormat::Json => print_json(&engine, file)?,
    }
    Ok(0)
}

fn print_human(engine: &HectorEngine, file: &Path) {
    for (id, check) in engine.checks() {
        let status = status_for(engine, id, file);
        let files = check.files.join(",");
        println!("{id}  {status}  files={files}  run={}", check.run);
    }
}

fn print_json(engine: &HectorEngine, file: &Path) -> Result<()> {
    let entries: Vec<ExplainEntry<'_>> = engine
        .checks()
        .iter()
        .map(|(id, check)| ExplainEntry {
            check: id,
            status: status_for(engine, id, file),
            files: &check.files,
            run: &check.run,
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}
