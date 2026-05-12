use crate::config::ContextScope;
use crate::engine::context::expand_context;
use crate::engine::{RuleContext, RuleEngine};
use crate::verdict::{Engine, Severity, Violation};
use anyhow::{anyhow, Result};

pub struct SemanticEngine;

impl RuleEngine for SemanticEngine {
    fn run(&self, ctx: &RuleContext) -> Result<Option<Violation>> {
        let llm = ctx.llm.ok_or_else(|| anyhow!("semantic engine requires LlmClient; provide via HectorEngine::builder().with_llm(...)"))?;
        let scope = ctx.rule.context.unwrap_or(ContextScope::Diff);
        let (primary, context_text) = expand_context(scope, ctx.diff, Some(ctx.file), ctx.cwd)?;
        let verdicts = llm.evaluate(
            &[(ctx.rule_id, ctx.rule)],
            &primary,
            context_text.as_deref(),
        )?;
        let total = verdicts.len();
        let Some(v) = verdicts.into_iter().find(|v| v.rule_id == ctx.rule_id) else {
            return Err(anyhow!(
                "LLM returned no verdict for rule `{}`; got {total} other verdicts",
                ctx.rule_id
            ));
        };
        match v.status {
            crate::llm::RuleStatus::Pass => Ok(None),
            crate::llm::RuleStatus::Violation { message, line } => {
                let severity = match ctx.rule.severity {
                    crate::config::Severity::Error => Severity::Error,
                    crate::config::Severity::Warning => Severity::Warning,
                };
                Ok(Some(Violation {
                    rule_id: ctx.rule_id.to_string(),
                    severity,
                    engine: Engine::Semantic,
                    file: ctx.file.display().to_string(),
                    line,
                    column: None,
                    message,
                    suggestion: ctx.rule.fix_hint.clone(),
                    context: None,
                }))
            }
        }
    }
}
