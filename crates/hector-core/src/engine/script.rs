use crate::config::Rule;
use crate::engine::capability::run_with_capabilities_env;
use crate::engine::{RuleContext, RuleEngine};
use crate::verdict::{Engine, Severity, Violation};
use anyhow::{anyhow, Result};
use std::path::Path;

pub struct ScriptEngine;

impl RuleEngine for ScriptEngine {
    fn run(&self, ctx: &RuleContext) -> Result<Vec<Violation>> {
        // P1-11: the trait now returns `Vec`. Script always produces at most
        // one violation per rule per file (it's a single subprocess), so we
        // wrap the existing `Option` outcome — empty vec for pass, one-element
        // vec for violation.
        Ok(run_script_rule_internal(
            ctx.rule_id,
            ctx.rule,
            ctx.file,
            ctx.diff.unwrap_or(""),
            ctx.cwd,
        )?
        .into_iter()
        .collect())
    }
}

/// Kept as a free function for backward compat with existing callsites.
pub fn run_script_rule(
    rule_id: &str,
    rule: &Rule,
    file: &Path,
    diff: &str,
    cwd: &Path,
) -> Result<Option<Violation>> {
    run_script_rule_internal(rule_id, rule, file, diff, cwd)
}

fn run_script_rule_internal(
    rule_id: &str,
    rule: &Rule,
    file: &Path,
    _diff: &str,
    cwd: &Path,
) -> Result<Option<Violation>> {
    let script = rule
        .script
        .as_ref()
        .ok_or_else(|| anyhow!("rule {rule_id} is engine: script but has no `script:` field"))?;
    // `{file}` expands to the shell parameter `"$HECTOR_FILE"`. The actual path
    // is passed via the child environment, never spliced into the command
    // text, so shell metacharacters in the filename cannot escape into the
    // surrounding command. The double-quotes prevent word-splitting on
    // whitespace in the path.
    let substituted = script.replace("{file}", "\"$HECTOR_FILE\"");
    let caps = rule.capabilities.clone().unwrap_or_default();
    let file_str = file.display().to_string();
    let outcome =
        run_with_capabilities_env(&substituted, cwd, &caps, &[("HECTOR_FILE", &file_str)])?;
    if outcome.exit_code == 0 {
        return Ok(None);
    }
    let severity = match rule.severity {
        crate::config::Severity::Error => Severity::Error,
        crate::config::Severity::Warning => Severity::Warning,
    };
    let message = if outcome.stderr.trim().is_empty() {
        outcome.stdout.trim().to_string()
    } else {
        outcome.stderr.trim().to_string()
    };
    Ok(Some(Violation {
        rule_id: rule_id.to_string(),
        severity,
        engine: Engine::Script,
        file: file.display().to_string(),
        line: None,
        column: None,
        message: if message.is_empty() {
            rule.description.clone()
        } else {
            message
        },
        suggestion: rule.fix_hint.clone(),
        context: None,
    }))
}
