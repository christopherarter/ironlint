//! `hector show-resolved-config` — print the post-extends merged gate set.
//!
//! Prints each gate in id order, annotated by the origin file it was defined
//! in. `tsv` (default) emits `gate_id<TAB>origin<TAB>files(comma-joined)<TAB>run`;
//! `yaml` and `json` emit a sequence of `{ gate, origin, files, run }`.
//! Read-only. Does not run any gate.

use crate::cli::ShowFormat;
use anyhow::Result;
use hector_core::config::Config;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
struct ResolvedGate {
    gate: String,
    origin: String,
    files: Vec<String>,
    run: String,
}

pub fn run(config: &Path, format: ShowFormat) -> Result<i32> {
    let (cfg, origins) = match hector_core::config::extends::resolve_with_origin(config) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            return Ok(1);
        }
    };
    let rows = build_rows(&cfg, &origins);
    match format {
        ShowFormat::Tsv => print_tsv(&rows),
        ShowFormat::Yaml => print_yaml(&rows)?,
        ShowFormat::Json => print_json(&rows)?,
    }
    Ok(0)
}

fn build_rows(cfg: &Config, origins: &BTreeMap<String, PathBuf>) -> Vec<ResolvedGate> {
    cfg.gates
        .iter()
        .map(|(id, gate)| ResolvedGate {
            gate: id.clone(),
            origin: origins
                .get(id)
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            files: gate.files.clone(),
            run: gate.run.clone(),
        })
        .collect()
}

fn print_tsv(rows: &[ResolvedGate]) {
    for r in rows {
        println!("{}\t{}\t{}\t{}", r.gate, r.origin, r.files.join(","), r.run);
    }
}

fn print_yaml(rows: &[ResolvedGate]) -> Result<()> {
    print!("{}", serde_yaml::to_string(rows)?);
    Ok(())
}

fn print_json(rows: &[ResolvedGate]) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(rows)?);
    Ok(())
}
