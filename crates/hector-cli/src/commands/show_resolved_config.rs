//! `hector show-resolved-config` — print the post-extends merged gate set.
//!
//! For each gate in id order:
//!   <gate-id>  (from <origin-path>)
//!     files: <comma-joined globs>
//!     run: <run>
//! Read-only. Does not run any gate.

use crate::cli::ShowFormat;
use anyhow::Result;
use std::path::Path;

pub fn run(config: &Path, _format: ShowFormat) -> Result<i32> {
    match hector_core::config::extends::resolve_with_origin(config) {
        Ok((cfg, origins)) => {
            for (id, gate) in &cfg.gates {
                let origin = origins
                    .get(id)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                println!("{id}  (from {origin})");
                println!("  files: {}", gate.files.join(","));
                println!("  run: {}", gate.run);
            }
            Ok(0)
        }
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            Ok(1)
        }
    }
}
