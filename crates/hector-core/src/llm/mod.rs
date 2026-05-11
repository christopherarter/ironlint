//! LLM client trait + provider impls (anthropic at later tasks).

pub mod no_llm;

use crate::config::Rule;
use anyhow::Result;

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

pub use no_llm::NoLlm;
