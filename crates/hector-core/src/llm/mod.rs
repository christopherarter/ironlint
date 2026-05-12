//! LLM client trait + provider impls.

pub mod anthropic;
pub mod openai_compat;
pub mod prompt;

use crate::config::{LlmConfig, Rule};
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

pub use anthropic::AnthropicClient;
pub use openai_compat::OpenAICompatClient;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleVerdict {
    pub rule_id: String,
    pub status: RuleStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleStatus {
    Pass,
    Violation { message: String, line: Option<u32> },
}

pub trait LlmClient: Send + Sync {
    fn evaluate(
        &self,
        rules: &[(&str, &Rule)],
        primary: &str,
        context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>>;
}

/// Construct an `LlmClient` from a parsed config's `llm:` block.
///
/// Returns `Ok(None)` (with a stderr warning) when a non-Ollama provider
/// needs an API key but the configured env var is missing. This lets
/// non-LLM rules (script, ast) still run when credentials are absent.
///
/// Errors when the provider name is unknown.
pub fn build_from_config(cfg: &LlmConfig) -> Result<Option<Box<dyn LlmClient>>> {
    let api_key = read_api_key(cfg);
    match cfg.provider.as_str() {
        "anthropic" => {
            let Some(key) = api_key else {
                return Ok(None);
            };
            Ok(Some(Box::new(AnthropicClient::new(
                key,
                &cfg.model,
                cfg.base_url.clone(),
            ))))
        }
        "openrouter" => {
            let Some(key) = api_key else {
                return Ok(None);
            };
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
            Ok(Some(Box::new(OpenAICompatClient::new(
                key, &cfg.model, base,
            ))))
        }
        "ollama" => {
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434/v1".to_string());
            let key = api_key.unwrap_or_default();
            Ok(Some(Box::new(OpenAICompatClient::new(
                key, &cfg.model, base,
            ))))
        }
        other => {
            bail!("unknown LLM provider `{other}`. Supported: anthropic, ollama, openrouter")
        }
    }
}

fn read_api_key(cfg: &LlmConfig) -> Option<String> {
    let env_name = cfg.api_key_env.as_deref()?;
    match std::env::var(env_name) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => {
            eprintln!(
                "hector: warning — env var `{env_name}` not set; provider `{}` requires it",
                cfg.provider
            );
            None
        }
    }
}

// ---- Wire-format helpers shared by Anthropic + OpenAI-compat clients ----

#[derive(Debug, Deserialize)]
struct WireVerdict {
    rule_id: String,
    status: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    line: Option<u32>,
}

/// Parse the LLM's JSON-array response into structured verdicts.
///
/// Tolerates extra prose around the array (some models wrap it in markdown
/// fences or a sentence).
pub(crate) fn parse_verdicts(text: &str) -> Result<Vec<RuleVerdict>> {
    let trimmed = text.trim();
    let start = trimmed
        .find('[')
        .ok_or_else(|| anyhow!("no JSON array in response: {trimmed}"))?;
    let end = trimmed
        .rfind(']')
        .ok_or_else(|| anyhow!("no closing bracket: {trimmed}"))?;
    let json = &trimmed[start..=end];
    let wire: Vec<WireVerdict> =
        serde_json::from_str(json).with_context(|| format!("parse verdict JSON: {json}"))?;
    Ok(wire
        .into_iter()
        .map(|w| RuleVerdict {
            rule_id: w.rule_id,
            status: match w.status.as_str() {
                "pass" => RuleStatus::Pass,
                "violation" => RuleStatus::Violation {
                    message: w.message.unwrap_or_default(),
                    line: w.line,
                },
                other => RuleStatus::Violation {
                    message: format!("unknown status from LLM: {other}"),
                    line: None,
                },
            },
        })
        .collect())
}
