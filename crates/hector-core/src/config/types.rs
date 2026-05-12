use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: u32,
    #[serde(default)]
    pub llm: Option<LlmConfig>,
    #[serde(default)]
    pub extends: Vec<String>,
    #[serde(default)]
    pub trust: Option<TrustBlock>,
    #[serde(default)]
    pub skip: Vec<String>,
    #[serde(default)]
    pub execution: Option<ExecutionConfig>,
    pub rules: BTreeMap<String, Rule>,
}

/// Optional execution-tuning block.
///
/// Controls the rayon pool that dispatches rules in parallel during
/// `HectorEngine::check`. Absence = use the default of
/// `min(8, num_cpus::get())`. The `HECTOR_MAX_WORKERS` env var overrides
/// any value set here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    /// Maximum worker threads. `0` clamps to 1 at pool-construction time
    /// (rayon rejects `num_threads(0)`).
    #[serde(default)]
    pub max_workers: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustBlock {
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub description: String,
    pub engine: EngineKind,
    #[serde(deserialize_with = "scope_one_or_many")]
    pub scope: Vec<String>,
    pub severity: Severity,

    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub context: Option<ContextScope>,
    #[serde(default)]
    pub capabilities: Option<Capabilities>,
    #[serde(default)]
    pub fix_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    Script,
    Ast,
    Semantic,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextScope {
    Diff,
    File,
    Repo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    pub network: bool,
    #[serde(default = "default_writes")]
    pub writes: WritesPolicy,
}

fn default_writes() -> WritesPolicy {
    WritesPolicy::None
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            network: false,
            writes: WritesPolicy::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WritesPolicy {
    None,
    CwdOnly,
    Tmp,
    Unrestricted,
}

fn scope_one_or_many<'de, D>(de: D) -> Result<Vec<String>, D::Error>
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
                    .ok_or_else(|| D::Error::custom("scope entry must be string"))
            })
            .collect(),
        _ => Err(D::Error::custom("scope must be string or list of strings")),
    }
}
