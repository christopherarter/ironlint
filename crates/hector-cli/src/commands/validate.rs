use anyhow::Result;
use hector_core::config::extends::resolve_trusted;
use std::path::Path;

pub fn run(config: &Path) -> Result<i32> {
    // Use the trust-verifying resolver so `validate` and `check` agree on what
    // "valid" means; otherwise a user can validate a config with an untrusted
    // parent and be surprised when `check` refuses to load it.
    match resolve_trusted(config) {
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
