use super::types::Config;
use anyhow::{anyhow, Context, Result};

/// Parse a `.hector.yml` (gates format).
///
/// Legacy v1/v2 configs (`schema_version:`, `rules:`, `engine:`) are rejected
/// with a curated message rather than serde's generic failure — hector 0.3
/// dropped the engine model. There is no migration path (no install base).
pub fn parse_str(input: &str) -> Result<Config> {
    if let Some(key) = legacy_marker(input) {
        return Err(anyhow!(
            "this looks like a pre-0.3 config (found `{key}:`). The 0.3 format uses a \
             top-level `gates:` map of `{{ files, run }}` entries — rewrite it. \
             See specs/2026-06-15-hector-gates-redesign-design.md"
        ));
    }
    let cfg: Config = serde_yaml::from_str(input).context("parsing hector config")?;
    Ok(cfg)
}

/// Return the first top-level legacy marker key present, if any.
fn legacy_marker(input: &str) -> Option<&'static str> {
    let value: serde_yaml::Value = serde_yaml::from_str(input).ok()?;
    let map = value.as_mapping()?;
    for key in ["schema_version", "rules", "trust"] {
        if map.contains_key(serde_yaml::Value::String(key.into())) {
            return Some(match key {
                "schema_version" => "schema_version",
                "rules" => "rules",
                _ => "trust",
            });
        }
    }
    None
}

pub fn parse_file(path: &std::path::Path) -> Result<Config> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_str(&content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_gates_config() {
        let cfg = parse_str("gates:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n").unwrap();
        assert!(cfg.gates.contains_key("g"));
    }

    #[test]
    fn rejects_legacy_schema_version() {
        let err = parse_str("schema_version: 2\nrules: {}\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("gates"),
            "error should point at the gates format: {err}"
        );
    }

    #[test]
    fn rejects_legacy_rules_block() {
        let err = parse_str("rules:\n  r:\n    engine: script\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("gates"),
            "error should point at the gates format: {err}"
        );
    }

    #[test]
    fn missing_gates_key_is_an_error() {
        assert!(parse_str("extends: []\n").is_err());
    }
}
