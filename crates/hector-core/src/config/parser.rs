use super::types::Config;
use anyhow::{anyhow, Context, Result};

pub const SUPPORTED_SCHEMAS: &[u32] = &[1, 2];

pub fn parse_str(input: &str) -> Result<Config> {
    let cfg: Config = serde_yaml::from_str(input).context("parsing hector config")?;
    if !SUPPORTED_SCHEMAS.contains(&cfg.schema_version) {
        return Err(anyhow!(
            "unsupported schema_version: {} (supported: {:?})",
            cfg.schema_version,
            SUPPORTED_SCHEMAS
        ));
    }
    Ok(cfg)
}

pub fn parse_file(path: &std::path::Path) -> Result<Config> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_str(&content)
}

/// Returns true if the parsed config is at the legacy schema (v1, bully).
/// Callers should log a one-time deprecation warning suggesting `hector migrate`.
pub fn is_legacy(cfg: &Config) -> bool {
    cfg.schema_version == 1
}
