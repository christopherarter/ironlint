use anyhow::{Context, Result};
use hector_core::trust;
use std::path::Path;

pub fn run(config: &Path) -> Result<i32> {
    let raw =
        std::fs::read_to_string(config).with_context(|| format!("reading {}", config.display()))?;
    let with_trust = trust::write_trust_block(&raw)?;
    std::fs::write(config, with_trust).with_context(|| format!("writing {}", config.display()))?;
    println!("trusted: {}", config.display());
    Ok(0)
}
