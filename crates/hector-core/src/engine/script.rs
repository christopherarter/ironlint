use crate::config::{OutputMode, Rule};
use crate::engine::capability::run_with_capabilities_env;
use crate::engine::output::{self, ParsedRecord};
use crate::engine::{RuleContext, RuleEngine};
use crate::verdict::{Engine, Severity, Violation};
use anyhow::{anyhow, Result};
use std::path::Path;

pub struct ScriptEngine;

impl RuleEngine for ScriptEngine {
    fn run(&self, ctx: &RuleContext) -> Result<Vec<Violation>> {
        run_script_rule(
            ctx.rule_id,
            ctx.rule,
            ctx.file,
            ctx.diff.unwrap_or(""),
            ctx.cwd,
        )
    }
}

/// Run a single script rule and return every violation it produced.
///
/// `output: passthrough` (default since R4, 2026-05-23) emits one violation
/// with the verbatim stdout+stderr in `message` and `line: None` — matches
/// bully and keeps pretty-printed linter frames intact.
/// `output: parsed` opts into [`output::parse`], which extracts structured
/// records (`file:line:col: msg`, `grep -n`, JSON) and emits one `Violation`
/// per record. Either way, an exit code of 0 still means "no violations."
pub fn run_script_rule(
    rule_id: &str,
    rule: &Rule,
    file: &Path,
    _diff: &str,
    cwd: &Path,
) -> Result<Vec<Violation>> {
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
        return Ok(Vec::new());
    }
    let severity = match rule.severity {
        crate::config::Severity::Error => Severity::Error,
        crate::config::Severity::Warning => Severity::Warning,
    };
    Ok(match rule.output {
        OutputMode::Parsed => {
            // Parsed mode preserves the historical "stderr if non-empty else
            // stdout" preference — canonical lint output (clippy, ruff,
            // eslint) lives on a single channel, and the parser is
            // line-oriented; mixing both streams would interleave noise.
            let raw = if outcome.stderr.trim().is_empty() {
                outcome.stdout.as_str()
            } else {
                outcome.stderr.as_str()
            };
            parsed_violations(rule_id, rule, file, raw, severity)
        }
        OutputMode::Passthrough => {
            // Passthrough mode (bully parity) emits both streams verbatim
            // so a script that already formats its own diagnostic doesn't
            // lose half its output to the channel-picker.
            passthrough_violation(
                rule_id,
                rule,
                file,
                &outcome.stdout,
                &outcome.stderr,
                severity,
            )
        }
    })
}

/// E2 parsed mode: one [`Violation`] per [`ParsedRecord`]. The empty-records
/// branch only fires when the script exited non-zero with no output at all on
/// the chosen stream — `output::parse` always returns ≥1 record for any
/// non-blank input via the fallback. We fabricate a description-only
/// violation in that case so the rule visibly fires; otherwise a script like
/// `[ -f forbidden.txt ] && exit 1` would silently disappear.
fn parsed_violations(
    rule_id: &str,
    rule: &Rule,
    file: &Path,
    raw: &str,
    severity: Severity,
) -> Vec<Violation> {
    let file_str = file.display().to_string();
    let records = output::parse(raw, &file_str);
    if records.is_empty() {
        return vec![empty_output_violation(rule_id, rule, &file_str, severity)];
    }
    records
        .into_iter()
        .map(|rec| record_to_violation(rule_id, rule, &file_str, rec, severity))
        .collect()
}

fn record_to_violation(
    rule_id: &str,
    rule: &Rule,
    fallback_file: &str,
    rec: ParsedRecord,
    severity: Severity,
) -> Violation {
    let message = if rec.message.is_empty() {
        rule.description.clone()
    } else {
        rec.message
    };
    Violation {
        rule_id: rule_id.to_string(),
        severity,
        engine: Engine::Script,
        file: if rec.file.is_empty() {
            fallback_file.to_string()
        } else {
            rec.file
        },
        line: rec.line,
        column: rec.column,
        message,
        suggestion: rule.fix_hint.clone(),
        context: None,
    }
}

/// E2 passthrough mode (bully parity): one violation, the combined
/// stdout+stderr in `message`, `line: None`, `column: None`.
///
/// The two streams are joined with a single newline, with empty/whitespace
/// streams filtered out so a tool that writes only to one channel doesn't
/// produce a leading or trailing blank line. If both streams are empty, the
/// rule's `description` stands in so the violation still says *something*.
fn passthrough_violation(
    rule_id: &str,
    rule: &Rule,
    file: &Path,
    stdout: &str,
    stderr: &str,
    severity: Severity,
) -> Vec<Violation> {
    let combined = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let message = if combined.is_empty() {
        rule.description.clone()
    } else {
        combined
    };
    vec![Violation {
        rule_id: rule_id.to_string(),
        severity,
        engine: Engine::Script,
        file: file.display().to_string(),
        line: None,
        column: None,
        message,
        suggestion: rule.fix_hint.clone(),
        context: None,
    }]
}

fn empty_output_violation(
    rule_id: &str,
    rule: &Rule,
    file_str: &str,
    severity: Severity,
) -> Violation {
    Violation {
        rule_id: rule_id.to_string(),
        severity,
        engine: Engine::Script,
        file: file_str.to_string(),
        line: None,
        column: None,
        message: rule.description.clone(),
        suggestion: rule.fix_hint.clone(),
        context: None,
    }
}
