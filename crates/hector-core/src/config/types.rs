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

/// When a check runs: `write` (on file edit) or `pre-commit` (on commit).
/// Defaults to `[write]` when omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Lifecycle {
    Write,
    PreCommit,
}

fn default_on() -> Vec<Lifecycle> {
    vec![Lifecycle::Write]
}

/// One step in a check's pipeline: an optional label and a shell command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub run: String,
}

/// A single check: files + (run xor steps) + on + name.
///
/// `run` is one-step sugar: `run: "cmd"` is equivalent to `steps: [{run: "cmd"}]`.
/// `on` defaults to `[write]`. The parser validates that exactly one of `run`
/// or `steps` is set. The path under check arrives as `$HECTOR_FILE`; proposed
/// content arrives on stdin. Commands are handed to `sh -c` verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    #[serde(deserialize_with = "files_one_or_many")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<Step>>,
    #[serde(default = "default_on")]
    pub on: Vec<Lifecycle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Check {
    /// The check's work as a step list. `run` is one-step sugar.
    /// Parser validation guarantees exactly one of run/steps is set.
    pub fn effective_steps(&self) -> Vec<Step> {
        if let Some(run) = &self.run {
            vec![Step {
                name: None,
                run: run.clone(),
            }]
        } else {
            self.steps.clone().unwrap_or_default()
        }
    }
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
        assert_eq!(g.run, Some("biome check".to_string()));
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

    // --- Phase 2: steps / on / name ---

    #[test]
    fn on_defaults_to_write() {
        let cfg: Config =
            serde_yaml::from_str("checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n").unwrap();
        assert_eq!(cfg.checks["g"].on, vec![Lifecycle::Write]);
    }

    #[test]
    fn lifecycle_parses_kebab_pre_commit() {
        let cfg: Config = serde_yaml::from_str(
            "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n    on: [write, pre-commit]\n",
        )
        .unwrap();
        assert_eq!(
            cfg.checks["g"].on,
            vec![Lifecycle::Write, Lifecycle::PreCommit]
        );
    }

    #[test]
    fn run_normalizes_to_one_step() {
        let cfg: Config =
            serde_yaml::from_str("checks:\n  g:\n    files: \"*\"\n    run: \"rustfmt\"\n")
                .unwrap();
        let steps = cfg.checks["g"].effective_steps();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].run, "rustfmt");
    }

    #[test]
    fn steps_list_parses_with_names() {
        let cfg: Config = serde_yaml::from_str(
            "checks:\n  g:\n    files: \"*\"\n    steps:\n      - name: a\n        run: \"true\"\n      - run: \"false\"\n",
        )
        .unwrap();
        let steps = cfg.checks["g"].effective_steps();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].name.as_deref(), Some("a"));
        assert_eq!(steps[1].name, None);
    }
}
