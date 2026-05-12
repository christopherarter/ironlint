use crate::config::Rule;
use crate::llm::{LlmClient, RuleStatus};
use crate::session_state::SessionState;
use crate::verdict::{Engine, Severity, Violation};
use anyhow::{anyhow, Result};

pub struct SessionEngine;

impl SessionEngine {
    pub fn evaluate(
        &self,
        state: &SessionState,
        rule_id: &str,
        rule: &Rule,
        llm: &dyn LlmClient,
    ) -> Result<Option<Violation>> {
        let aggregated = state
            .edits
            .iter()
            .map(|e| format!("--- file: {} ---\n{}", e.file, e.diff))
            .collect::<Vec<_>>()
            .join("\n\n");
        let verdicts = llm.evaluate(&[(rule_id, rule)], &aggregated, None)?;
        let total = verdicts.len();
        let Some(v) = verdicts.into_iter().find(|v| v.rule_id == rule_id) else {
            return Err(anyhow!(
                "LLM returned no verdict for rule `{rule_id}`; got {total} other verdicts"
            ));
        };
        match v.status {
            RuleStatus::Pass => Ok(None),
            RuleStatus::Violation { message, line } => {
                let severity = match rule.severity {
                    crate::config::Severity::Error => Severity::Error,
                    crate::config::Severity::Warning => Severity::Warning,
                };
                Ok(Some(Violation {
                    rule_id: rule_id.to_string(),
                    severity,
                    engine: Engine::Session,
                    file: "".to_string(),
                    line,
                    column: None,
                    message,
                    suggestion: rule.fix_hint.clone(),
                    context: None,
                }))
            }
        }
    }
}
