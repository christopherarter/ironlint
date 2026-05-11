use anyhow::Result;
use hector_core::config::parse_file_with_extends;
use std::path::Path;

pub fn run(config: &Path) -> Result<i32> {
    match parse_file_with_extends(config) {
        Ok(_) => {
            println!("OK: {} is valid", config.display());
            Ok(0)
        }
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            Ok(1)
        }
    }
}
