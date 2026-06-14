//! Engine module: the RuleEngine trait, RuleContext, and per-engine impls.

pub mod ast;
pub mod capability;
pub mod output;
pub mod script;

use crate::config::Rule;
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
}

pub trait RuleEngine: Send + Sync {
    /// Evaluate `ctx` and return every violation produced by this engine.
    ///
    /// Returning `Ok(Vec::new())` means "this rule passed for this file".
    /// The script engine emits at most one verdict per call (a one-element
    /// vec); the AST engine can hit many sites in one file and emits one
    /// entry per match.
    fn run(&self, ctx: &RuleContext) -> Result<Vec<Violation>>;
}
