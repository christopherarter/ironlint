use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub extends: Vec<String>,
    #[serde(default)]
    pub execution: ExecutionConfig,
    pub checks: BTreeMap<String, Check>,
}

/// Optional execution-tuning block.
///
/// `timeout_secs` bounds each check's wall-clock; a check that exceeds it is
/// killed and reported as InternalError (never a silent pass). The
/// `HECTOR_TIMEOUT` env var overrides this at run time. Dispatch is
/// sequential; parallelism tuning is not exposed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionConfig {
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_timeout_secs() -> u64 {
    30
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            timeout_secs: default_timeout_secs(),
        }
    }
}

/// A single check: match `files`, run `run`, read its exit code.
///
/// `run` is handed to the shell verbatim — no `{file}`/`{path}` templating.
/// The path under check arrives as `$HECTOR_FILE`; proposed content arrives
/// on stdin. `run` may be an inline command or a path to a script.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    #[serde(deserialize_with = "files_one_or_many")]
    pub files: Vec<String>,
    pub run: String,
}

/// Accept either a single glob string or a list of globs for `files`.
/// Mirrors the old `scope` deserializer (bully parity).
fn files_one_or_many<'de, D>(de: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let v = serde_yaml::Value::deserialize(de)?;
    match v {
        serde_yaml::Value::String(s) => Ok(vec![s]),
        serde_yaml::Value::Sequence(seq) => seq
            .into_iter()
            .map(|x| {
                x.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| D::Error::custom("files entry must be string"))
            })
            .collect(),
        _ => Err(D::Error::custom("files must be string or list of strings")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_check_with_files_list() {
        let cfg: Config = serde_yaml::from_str(
            "checks:\n  biome:\n    files: [\"**/*.ts\"]\n    run: \"biome check\"\n",
        )
        .unwrap();
        let g = cfg.checks.get("biome").unwrap();
        assert_eq!(g.files, vec!["**/*.ts".to_string()]);
        assert_eq!(g.run, "biome check");
    }

    #[test]
    fn files_accepts_a_bare_string() {
        let cfg: Config =
            serde_yaml::from_str("checks:\n  g:\n    files: \"**/*.rs\"\n    run: \"true\"\n")
                .unwrap();
        assert_eq!(cfg.checks["g"].files, vec!["**/*.rs".to_string()]);
    }

    #[test]
    fn execution_timeout_defaults_to_30() {
        let cfg: Config =
            serde_yaml::from_str("checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n").unwrap();
        assert_eq!(cfg.execution.timeout_secs, 30);
    }

    #[test]
    fn execution_timeout_is_overridable() {
        let cfg: Config = serde_yaml::from_str(
            "execution:\n  timeout_secs: 5\nchecks:\n  g:\n    files: \"*\"\n    run: \"true\"\n",
        )
        .unwrap();
        assert_eq!(cfg.execution.timeout_secs, 5);
    }

    #[test]
    fn extends_defaults_to_empty() {
        let cfg: Config =
            serde_yaml::from_str("checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n").unwrap();
        assert!(cfg.extends.is_empty());
    }
}
