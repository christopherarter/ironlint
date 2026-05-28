use super::{parse_verdicts, LlmClient, RuleVerdict};
use crate::config::Rule;
use crate::llm::prompt::build_prompt_split;
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::time::Duration;

/// Wall-clock budget for a single Anthropic request.
///
/// Without this, a hung endpoint blocks the entire `check` call indefinitely.
/// 30s is generous for a single-shot completion at our token budget;
/// long-running rules should be redesigned, not waited on.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct AnthropicClient {
    api_key: String,
    model: String,
    base_url: String,
    client: reqwest::blocking::Client,
}

impl AnthropicClient {
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<String>,
    ) -> Self {
        // `Client::builder().build()` only fails on TLS setup issues, which are
        // not environment-dependent in our build (rustls is statically linked).
        // Keep `new` infallible so callers don't need to thread a Result for a
        // configuration we control end-to-end.
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("static reqwest client build");
        Self {
            api_key: api_key.into(),
            model: model.into(),
            base_url: base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string()),
            client,
        }
    }

    pub fn from_env(model: &str) -> Result<Self> {
        let key = std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY not set")?;
        Ok(Self::new(key, model, None))
    }
}

#[derive(Debug, Deserialize)]
struct Message {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

impl LlmClient for AnthropicClient {
    fn evaluate(
        &self,
        rules: &[(&str, &Rule)],
        primary: &str,
        context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>> {
        // Anthropic's `/v1/messages` accepts a top-level `system` parameter
        // processed separately from the conversation. Operator-authored policy
        // and output-format instructions go there; only attacker-controlled
        // evidence goes in the user message. The model can be trained to
        // weight `system` over `user`, widening the trust boundary beyond what
        // the sentinel tags alone guarantee.
        let (system, user) = build_prompt_split(rules, primary, context);
        let url = format!("{}/v1/messages", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": system,
            "messages": [{ "role": "user", "content": user }],
        });
        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .context("anthropic request")?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().unwrap_or_default();
            // A misconfigured debug proxy or echo endpoint may return our own
            // Bearer/API key in the body. Truncate to a debug-sized slice and
            // redact secret-shaped tokens before bubbling up.
            let safe = super::sanitize_error_body(&text);
            return Err(anyhow!("anthropic returned {status}: {safe}"));
        }
        let message: Message = response.json().context("parse anthropic response")?;
        let text = message
            .content
            .iter()
            .find(|b| b.block_type == "text")
            .and_then(|b| b.text.as_ref())
            .ok_or_else(|| anyhow!("anthropic response missing text content"))?;
        parse_verdicts(text)
    }
}
