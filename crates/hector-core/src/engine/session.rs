use crate::config::Rule;
use crate::llm::{LlmClient, RuleStatus};
use crate::session_state::SessionState;
use crate::verdict::{Engine, Severity, Violation};
use anyhow::{anyhow, Result};

/// B3: build the session-aggregate diff string from all edits in `state`.
///
/// Each edit is wrapped in a framing delimiter that embeds the
/// `session_id` to prevent attacker-controlled diff content from forging
/// a frame for a different file (P1-9). The framing uses `timestamp` and
/// `diff` from [`crate::session_state::EditRecord`]; there is no `tool`
/// field on the record.
///
/// Called by both [`SessionEngine::evaluate`] (direct-LLM path) and the
/// runner's `check_session_with_options` (deferred envelope path) so the
/// two routes produce byte-identical aggregated evidence.
pub fn framed_aggregate(state: &SessionState) -> String {
    state
        .edits
        .iter()
        .map(|e| {
            format!(
                "<<<EDIT {session_id}/{file}>>>\n{ts}\n{diff}\n<<<END EDIT>>>\n",
                session_id = state.session_id,
                file = e.file,
                ts = e.timestamp,
                diff = e.diff,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct SessionEngine;

impl SessionEngine {
    pub fn evaluate(
        &self,
        state: &SessionState,
        rule_id: &str,
        rule: &Rule,
        llm: &dyn LlmClient,
    ) -> Result<Option<Violation>> {
        // P1-9: bind the per-edit framing delimiter to the random
        // `session_id` so attacker-controlled diff content cannot forge
        // a frame for a different file. The legacy boundary
        // `--- file: <path> ---` was trivially reproducible inside any
        // edit's diff; the session id makes the boundary unpredictable.
        //
        // B3: delegate to `framed_aggregate` so the LLM path and the
        // deferred-envelope path produce byte-identical evidence.
        let aggregated = framed_aggregate(state);
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
