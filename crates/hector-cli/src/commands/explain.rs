//! `hector explain <file>` — read-only gate applicability report.
//!
//! For each gate in the resolved config (BTreeMap id order), prints one line:
//!   <gate-id>  <match|skip>  files=<comma-joined globs>  run=<run>
//! where `match` iff the gate's file globs apply to the given path.
//! No gate logic is executed. Errors go to stderr; exit 1.

use crate::cli::OutputFormat;
use anyhow::Result;
use hector_core::runner::HectorEngine;
use std::path::Path;

pub fn run(file: &Path, _format: OutputFormat, config: &Path) -> Result<i32> {
    let engine = match HectorEngine::load(config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            return Ok(1);
        }
    };
    for (id, gate) in engine.gates() {
        let status = if engine.gate_matches_path(id, file) {
            "match"
        } else {
            "skip"
        };
        let files = gate.files.join(",");
        println!("{id}  {status}  files={files}  run={}", gate.run);
    }
    Ok(0)
}
