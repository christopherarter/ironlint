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
    /// Required at load time for direct-API providers (`anthropic`,
    /// `openrouter`, `ollama`); optional for `provider: claude-code-subagent`,
    /// which never reads it — the in-session subagent uses the Claude Code
    /// session's model. Validation lives in `runner::HectorEngine::load`, not
    /// the serde derivation, so the missing-model error can name the provider
    /// explicitly rather than surfacing a generic serde "missing field"
    /// diagnostic.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    /// Optional model id propagated through the deferred envelope so the
    /// Claude Code skill can dispatch the `hector-evaluator` subagent under a
    /// specific model (e.g. `haiku` for cheap policy checks). Free-form:
    /// Claude Code's subagent dispatch validates the value at the right layer.
    /// Ignored when `provider != claude-code-subagent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator_model: Option<String>,
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

    /// How to interpret the script's stdout/stderr (bully parity).
    ///
    /// `Passthrough` (the default) keeps the chosen stream verbatim in
    /// `Violation.message` with `line: None` — matches bully's design and
    /// avoids mis-parsing pretty-printed linter frames (biome,
    /// eslint pretty, clippy default, …) as chains of false violations.
    /// `Parsed` is opt-in for the formats we explicitly support
    /// ([`crate::engine::output::parse`] handles canonical
    /// `file:line:col: msg` from ruff / eslint --format compact /
    /// clippy --message-format short, the `grep -n` `<line>:<text>` shape,
    /// and JSON objects/arrays). The supported-format set does not grow —
    /// we will not chase a parser per linter.
    ///
    /// Only consulted by the script engine. Other engines ignore this
    /// field — they construct violations from in-process structure.
    #[serde(default)]
    pub output: OutputMode,
}

/// Per-rule script-output interpretation.
///
/// Default is [`OutputMode::Passthrough`] — matches bully and keeps
/// unconfigured rules safe against pretty-printed linter frames.
/// [`OutputMode::Parsed`] is opt-in; see the [`Rule::output`] docs for the
/// list of formats it knows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    Parsed,
    #[default]
    Passthrough,
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
