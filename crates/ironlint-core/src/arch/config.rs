use anyhow::{bail, Result};
use globset::Glob;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchConfig {
    pub layers: Vec<LayerDecl>,
    #[serde(default)]
    pub rules: Vec<RuleDecl>,
    #[serde(default)]
    pub ignore: Vec<String>,
}

/// A named layer: `presentation: ["src/components/**", ...]`. Parsed from a
/// YAML mapping (name → glob list), so order = insertion order (deterministic
/// first-match layer classification).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerDecl {
    pub name: String,
    pub globs: Vec<String>,
}

/// `from X may_import [Y, Z]`. A layer with no rule entry may import any layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDecl {
    pub from: String,
    pub may_import: Vec<String>,
}

impl ArchConfig {
    pub fn validate(&self) -> Result<()> {
        if self.layers.is_empty() {
            bail!("architecture.layers must contain at least one layer");
        }

        let mut layer_names = HashSet::new();
        for layer in &self.layers {
            if layer.name.trim().is_empty() || !layer_names.insert(layer.name.as_str()) {
                bail!("duplicate or empty architecture layer `{}`", layer.name);
            }
            if layer.globs.is_empty() {
                bail!(
                    "architecture layer `{}` must declare at least one glob",
                    layer.name
                );
            }
            for glob in &layer.globs {
                Glob::new(glob).map_err(|e| {
                    anyhow::anyhow!("invalid glob `{glob}` in layer `{}`: {e}", layer.name)
                })?;
            }
        }

        let mut rule_sources = HashSet::new();
        for rule in &self.rules {
            if !layer_names.contains(rule.from.as_str()) {
                bail!(
                    "architecture rule references unknown source layer `{}`",
                    rule.from
                );
            }
            if !rule_sources.insert(rule.from.as_str()) {
                bail!("duplicate rule for architecture layer `{}`", rule.from);
            }
            for target in &rule.may_import {
                if !layer_names.contains(target.as_str()) {
                    bail!(
                        "architecture rule `{}` references unknown target layer `{target}`",
                        rule.from
                    );
                }
            }
        }
        for glob in &self.ignore {
            Glob::new(glob)
                .map_err(|e| anyhow::anyhow!("invalid architecture ignore glob `{glob}`: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_arch_config() {
        let yaml = "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\nrules:\n  - from: presentation\n    may_import: [data]\nignore: [\"**/*.test.*\"]\n";
        let cfg: ArchConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.layers.len(), 2);
        assert_eq!(cfg.layers[0].name, "presentation");
        assert_eq!(cfg.rules[0].from, "presentation");
        assert_eq!(cfg.rules[0].may_import, vec!["data".to_string()]);
        assert_eq!(cfg.ignore, vec!["**/*.test.*".to_string()]);
    }

    #[test]
    fn rules_and_ignore_default_to_empty() {
        let yaml = "layers:\n  - name: x\n    globs: [\"*\"]\n";
        let cfg: ArchConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.rules.is_empty());
        assert!(cfg.ignore.is_empty());
    }

    #[test]
    fn rejects_unknown_field() {
        let yaml = "layers: []\nfoo: bar\n";
        let err = serde_yaml::from_str::<ArchConfig>(yaml)
            .unwrap_err()
            .to_string();
        assert!(err.contains("foo"), "{err}");
    }

    #[test]
    fn rejects_unknown_and_duplicate_rule_layers() {
        let unknown: ArchConfig = serde_yaml::from_str(
            "layers:\n  - name: presentation\n    globs: [\"src/**\"]\nrules:\n  - from: presentaton\n    may_import: []\n",
        )
        .unwrap();
        assert!(unknown
            .validate()
            .unwrap_err()
            .to_string()
            .contains("presentaton"));

        let duplicate: ArchConfig = serde_yaml::from_str(
            "layers:\n  - name: presentation\n    globs: [\"src/**\"]\nrules:\n  - from: presentation\n    may_import: []\n  - from: presentation\n    may_import: []\n",
        )
        .unwrap();
        assert!(duplicate
            .validate()
            .unwrap_err()
            .to_string()
            .contains("duplicate rule"));
    }

    #[test]
    fn rejects_duplicate_layers_and_invalid_globs() {
        let duplicate: ArchConfig = serde_yaml::from_str(
            "layers:\n  - name: data\n    globs: [\"src/data/**\"]\n  - name: data\n    globs: [\"src/other/**\"]\n",
        )
        .unwrap();
        assert!(duplicate
            .validate()
            .unwrap_err()
            .to_string()
            .contains("duplicate or empty architecture layer"));

        let bad_glob: ArchConfig =
            serde_yaml::from_str("layers:\n  - name: data\n    globs: [\"[\"]\n").unwrap();
        assert!(bad_glob.validate().is_err());
    }
}
