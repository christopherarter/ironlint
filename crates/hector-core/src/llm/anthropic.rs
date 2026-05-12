use super::{parse_verdicts, LlmClient, RuleVerdict};
use crate::config::Rule;
use crate::llm::prompt::build_prompt;
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

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
        Self {
            api_key: api_key.into(),
            model: model.into(),
            base_url: base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string()),
            client: reqwest::blocking::Client::new(),
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
        let prompt = build_prompt(rules, primary, context);
        let url = format!("{}/v1/messages", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 4096,
            "messages": [{ "role": "user", "content": prompt }],
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
            return Err(anyhow!("anthropic returned {status}: {text}"));
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
