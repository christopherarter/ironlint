use crate::cli::OutputFormat;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub fn run(
    _file: Option<PathBuf>,
    _diff: Option<PathBuf>,
    _format: OutputFormat,
    _config: &Path,
) -> Result<i32> {
    eprintln!("check: not yet implemented");
    Ok(1)
}
