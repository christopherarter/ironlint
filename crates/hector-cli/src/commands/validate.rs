use anyhow::Result;
use std::path::Path;

pub fn run(config: &Path) -> Result<i32> {
    match hector_core::config::parse_file_with_extends(config) {
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
