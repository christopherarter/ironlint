//! OpenAI-compatible chat-completions client.
//!
//! Speaks the OpenAI `/chat/completions` endpoint shape, which is the
//! lingua franca for self-hosted and aggregator providers — Ollama
//! (`http://localhost:11434/v1`), OpenRouter (`https://openrouter.ai/api/v1`),
//! and OpenAI itself (`https://api.openai.com/v1`).
//!
//! The Authorization header is only sent when `api_key` is non-empty, so
//! Ollama (which doesn't require auth) works without setting one.

use super::{parse_verdicts, LlmClient, RuleVerdict};
use crate::config::Rule;
use crate::llm::prompt::build_prompt;
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::time::Duration;

/// Wall-clock budget for a single OpenAI-compatible request.
///
/// Without this, a hung Ollama / OpenRouter / OpenAI endpoint blocks the
/// entire `check` call indefinitely. 30s is generous for a single-shot
/// completion; long-running rules should be redesigned, not waited on.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct OpenAICompatClient {
    api_key: String,
    model: String,
    base_url: String,
    client: reqwest::blocking::Client,
}

impl OpenAICompatClient {
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
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
            base_url: base_url.into(),
            client,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

impl LlmClient for OpenAICompatClient {
    fn evaluate(
        &self,
        rules: &[(&str, &Rule)],
        primary: &str,
        context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>> {
        let prompt = build_prompt(rules, primary, context);
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.model,
            "messages": [{ "role": "user", "content": prompt }],
        });
        let mut req = self.client.post(&url).json(&body);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let response = req.send().context("openai-compat request")?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().unwrap_or_default();
            // Aggregator proxies (OpenRouter, etc.) and Ollama debug builds
            // occasionally echo the Authorization header into 5xx bodies.
            // Truncate + redact before propagating.
            let safe = super::sanitize_error_body(&text);
            return Err(anyhow!("openai-compat server returned {status}: {safe}"));
        }
        let payload: ChatResponse = response.json().context("parse openai-compat response")?;
        let text = payload
            .choices
            .first()
            .and_then(|c| c.message.content.as_deref())
            .ok_or_else(|| anyhow!("openai-compat response missing content"))?;
        parse_verdicts(text)
    }
}
