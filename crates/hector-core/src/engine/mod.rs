//! Engine module: the RuleEngine trait, RuleContext, and per-engine impls.

pub mod ast;
pub mod capability;
pub mod context;
pub mod script;
pub mod semantic;
pub mod session;

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
    /// Evaluate `ctx` and return every violation produced by this engine.
    ///
    /// Returning `Ok(Vec::new())` means "this rule passed for this file".
    /// Engines that conceptually emit at most one verdict per call (script,
    /// semantic) wrap their single outcome in a one-element vec; engines that
    /// can hit many sites in one file (AST) emit one entry per match.
    fn run(&self, ctx: &RuleContext) -> Result<Vec<Violation>>;
}
