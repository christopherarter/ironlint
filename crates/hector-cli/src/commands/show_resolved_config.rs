//! `hector show-resolved-config` — print the post-extends merged check set.
//!
//! Prints each check in id order, annotated by the origin file it was defined
//! in. `tsv` (default) emits `check_id<TAB>origin<TAB>files(comma-joined)<TAB>run`;
//! `yaml` and `json` emit a sequence of `{ check, origin, files, run }`.
//! Read-only. Does not run any check.

use crate::cli::ShowFormat;
use anyhow::Result;
use hector_core::config::Config;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
struct ResolvedCheck {
    check: String,
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

fn build_rows(cfg: &Config, origins: &BTreeMap<String, PathBuf>) -> Vec<ResolvedCheck> {
    cfg.checks
        .iter()
        .map(|(id, check)| ResolvedCheck {
            check: id.clone(),
            origin: origins
                .get(id)
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            files: check.files.clone(),
            run: check.run.clone(),
        })
        .collect()
}

fn print_tsv(rows: &[ResolvedCheck]) {
    for r in rows {
        println!(
            "{}\t{}\t{}\t{}",
            r.check,
            r.origin,
            r.files.join(","),
            r.run
        );
    }
}

fn print_yaml(rows: &[ResolvedCheck]) -> Result<()> {
    print!("{}", serde_yaml::to_string(rows)?);
    Ok(())
}

fn print_json(rows: &[ResolvedCheck]) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(rows)?);
    Ok(())
}
