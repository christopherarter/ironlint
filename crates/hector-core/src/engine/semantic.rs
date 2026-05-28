use crate::config::ContextScope;
use crate::engine::context::expand_context;
use crate::engine::{RuleContext, RuleEngine};
use crate::verdict::{Engine, Severity, Violation};
use anyhow::{anyhow, Result};

pub struct SemanticEngine;

impl RuleEngine for SemanticEngine {
    fn run(&self, ctx: &RuleContext) -> Result<Vec<Violation>> {
        let llm = ctx.llm.ok_or_else(|| anyhow!("semantic engine requires LlmClient; provide via HectorEngine::builder().with_llm(...)"))?;
        let scope = ctx.rule.context.unwrap_or(ContextScope::Diff);
        let (primary, context_text) = expand_context(scope, ctx.diff, Some(ctx.file), ctx.cwd)?;
        let verdicts = llm.evaluate(
            &[(ctx.rule_id, ctx.rule)],
            &primary,
            context_text.as_deref(),
        )?;
        let total = verdicts.len();
        // The LLM must return a verdict whose `rule_id` matches what we asked
        // about. If it hallucinates a different id, surface that as an engine
        // error rather than silently passing.
        let Some(v) = verdicts.into_iter().find(|v| v.rule_id == ctx.rule_id) else {
            return Err(anyhow!(
                "LLM returned no verdict for rule `{}`; got {total} other verdicts",
                ctx.rule_id
            ));
        };
        // Semantic emits at most one violation per call: empty vec means pass,
        // one-element vec means violation. Matches script — only AST is
        // multi-element today.
        match v.status {
            crate::llm::RuleStatus::Pass => Ok(Vec::new()),
            crate::llm::RuleStatus::Violation { message, line } => {
                let severity = match ctx.rule.severity {
                    crate::config::Severity::Error => Severity::Error,
                    crate::config::Severity::Warning => Severity::Warning,
                };
                Ok(vec![Violation {
                    rule_id: ctx.rule_id.to_string(),
                    severity,
                    engine: Engine::Semantic,
                    file: ctx.file.display().to_string(),
                    line,
                    column: None,
                    message,
                    suggestion: ctx.rule.fix_hint.clone(),
                    context: None,
                }])
            }
        }
    }
}
