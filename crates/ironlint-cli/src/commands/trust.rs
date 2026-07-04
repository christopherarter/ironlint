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
    ironlint_core::trust::bless(&config)?;
    println!("trusted: {}", config.display());
    Ok(0)
}
