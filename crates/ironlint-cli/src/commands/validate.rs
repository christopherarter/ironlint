use crate::cli::OutputFormat;
use anyhow::Result;
use serde_json::json;
use std::path::Path;

pub fn run(config: &Path, format: OutputFormat) -> Result<i32> {
    let config = match crate::commands::config::resolve_config(config) {
        Ok(p) => p,
        Err(msg) => {
            return Ok(crate::commands::error_report::emit_error(format, &msg, 1));
        }
    };
    match ironlint_core::config::parse_file_with_extends(&config) {
        Ok(cfg) => {
            match format {
                OutputFormat::Human => println!("ok: {} check(s)", cfg.checks.len()),
                OutputFormat::Json => {
                    let body = json!({ "status": "ok", "checks": cfg.checks.len() });
                    println!("{body}");
                }
            }
            Ok(0)
        }
        Err(e) => Ok(crate::commands::error_report::emit_error(
            format,
            &format!("{e:#}"),
            1,
        )),
    }
}
