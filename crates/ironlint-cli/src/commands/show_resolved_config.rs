//! `ironlint show-resolved-config` — print the post-extends merged check set.
//!
//! Prints each check in id order, annotated by the origin file it was defined
//! in. `tsv` (default) emits `check_id<TAB>origin<TAB>files(comma-joined)<TAB>run`;
//! `yaml` and `json` emit a sequence of `{ check, origin, files, run }`.
//! Read-only. Does not run any check.

use crate::cli::ShowFormat;
use anyhow::Result;
use ironlint_core::config::Config;
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
    let config = match crate::commands::config::resolve_config(config) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("error: {msg}");
            return Ok(1);
        }
    };
    let (mut cfg, origins) = match ironlint_core::config::extends::resolve_with_origin(&config) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {:#}", e);
            return Ok(1);
        }
    };
    if let Err(e) = ironlint_core::arch::lowering::lower_architecture(&mut cfg) {
        eprintln!("error: {:#}", e);
        return Ok(1);
    }
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
            origin: if id == "__arch__" {
                "<architecture>".to_string()
            } else {
                origins
                    .get(id)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            },
            files: check.files.clone(),
            run: check.run.clone().unwrap_or_default(),
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
