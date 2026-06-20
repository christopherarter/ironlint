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

/// Read just the `schema_version` field without enforcing the full v2 shape.
///
/// Used by the runner to detect v1 *before* trust verification so we can emit
/// a friendly "run `hector migrate`" hint instead of the generic
/// "trust block missing" error.
///
/// Returns `None` for any input that does not have a parseable integer
/// `schema_version` at the top level — the normal load path will then surface
/// a proper parse error.
pub fn peek_schema_version(input: &str) -> Option<u32> {
    let v: serde_yaml::Value = serde_yaml::from_str(input).ok()?;
    v.get("schema_version")?
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
}
