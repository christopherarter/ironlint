use anyhow::Result;
use std::path::Path;

pub fn run(config: &Path) -> Result<i32> {
    let config = match crate::commands::config::resolve_config(config) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("ERROR: {msg}");
            return Ok(1);
        }
    };
    match ironlint_core::config::parse_file_with_extends(&config) {
        Ok(cfg) => {
            println!("ok: {} check(s)", cfg.checks.len());
            Ok(0)
        }
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            Ok(1)
        }
    }
}
