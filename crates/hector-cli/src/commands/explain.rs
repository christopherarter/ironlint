//! `hector explain <file>` — read-only gate applicability report.
//!
//! For each gate in the resolved config (BTreeMap id order), reports whether
//! the gate's file globs apply to the given path (`match`) or not (`skip`).
//! `human` (default) prints one line per gate
//! `<gate-id>  <match|skip>  files=<globs>  run=<run>`; `json` prints an array
//! of `{ gate, status, files, run }` objects. No gate logic is executed.
//! Errors go to stderr; exit 1.

use crate::cli::OutputFormat;
use anyhow::Result;
use hector_core::runner::HectorEngine;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
struct ExplainEntry<'a> {
    gate: &'a str,
    status: &'static str,
    files: &'a [String],
    run: &'a str,
}

fn status_for(engine: &HectorEngine, gate_id: &str, file: &Path) -> &'static str {
    if engine.gate_matches_path(gate_id, file) {
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
    for (id, gate) in engine.gates() {
        let status = status_for(engine, id, file);
        let files = gate.files.join(",");
        println!("{id}  {status}  files={files}  run={}", gate.run);
    }
}

fn print_json(engine: &HectorEngine, file: &Path) -> Result<()> {
    let entries: Vec<ExplainEntry<'_>> = engine
        .gates()
        .iter()
        .map(|(id, gate)| ExplainEntry {
            gate: id,
            status: status_for(engine, id, file),
            files: &gate.files,
            run: &gate.run,
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}
