//! Engine module: the RuleEngine trait, RuleContext, and per-engine impls.

pub mod ast;
pub mod capability;
pub mod script;

use crate::config::Rule;
use crate::llm::LlmClient;
use crate::verdict::Violation;
use anyhow::Result;
use std::path::Path;

pub struct RuleContext<'a> {
    pub rule_id: &'a str,
    pub rule: &'a Rule,
    pub file: &'a Path,
    pub content: Option<&'a str>,
    pub diff: Option<&'a str>,
    pub cwd: &'a Path,
    pub llm: Option<&'a dyn LlmClient>,
}

pub trait RuleEngine: Send + Sync {
    fn run(&self, ctx: &RuleContext) -> Result<Option<Violation>>;
}
