use anyhow::Result;
use std::path::Path;

pub fn run(config: &Path) -> Result<i32> {
    hector_core::trust::bless(config)?;
    println!("trusted: {}", config.display());
    Ok(0)
}
