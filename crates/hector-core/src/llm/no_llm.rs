use super::{LlmClient, RuleVerdict};
use crate::config::Rule;
use anyhow::{bail, Result};

/// LlmClient used when no LLM provider is configured. Errors if called.
pub struct NoLlm;

impl LlmClient for NoLlm {
    fn evaluate(
        &self,
        _rules: &[(&str, &Rule)],
        _primary: &str,
        _context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>> {
        bail!("no LLM client configured but a semantic/session rule was evaluated; provide `llm:` block in config or inject via HectorEngine::builder")
    }
}
