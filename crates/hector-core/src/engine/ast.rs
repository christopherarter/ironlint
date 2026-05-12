//! AST engine implementation via the `ast_grep_core` library.
//!
//! Uses `ast-grep-core` + `ast-grep-language` crates (v0.42) to perform
//! structural pattern matching directly in-process (no CLI subprocess).

use crate::engine::{RuleContext, RuleEngine};
use crate::verdict::{Engine, Severity, Violation};
use anyhow::{anyhow, Result};

pub struct AstEngine;

impl RuleEngine for AstEngine {
    fn run(&self, ctx: &RuleContext) -> Result<Vec<Violation>> {
        let pattern_str = ctx.rule.pattern.as_ref().ok_or_else(|| {
            anyhow!(
                "rule {} is engine: ast but has no `pattern:` field",
                ctx.rule_id
            )
        })?;
        let lang_name = ctx
            .rule
            .language
            .as_ref()
            .ok_or_else(|| anyhow!("rule {} is engine: ast but has no `language:` field (inference not implemented in 0.1b)", ctx.rule_id))?;
        let content = ctx
            .content
            .ok_or_else(|| anyhow!("ast engine requires file content (CheckInput::File)"))?;

        let severity = match ctx.rule.severity {
            crate::config::Severity::Error => Severity::Error,
            crate::config::Severity::Warning => Severity::Warning,
        };

        // P1-11: emit one violation per matched node, not just the first one.
        // The previous `Option<Violation>` return shape forced us to drop
        // every match after the first; `Vec<Violation>` is the fix.
        let matches = find_all_matches(content, pattern_str, lang_name)?;
        let mut violations = Vec::with_capacity(matches.len());
        for (line, column, context_str) in matches {
            violations.push(Violation {
                rule_id: ctx.rule_id.to_string(),
                severity,
                engine: Engine::Ast,
                file: ctx.file.display().to_string(),
                line: Some(line),
                column: Some(column),
                message: ctx.rule.description.clone(),
                suggestion: ctx.rule.fix_hint.clone(),
                context: Some(context_str),
            });
        }
        Ok(violations)
    }
}

/// Number of lines on either side of an AST match included in
/// `Violation.context`. The match line plus this many lines above and below
/// give consumers (editors, CI annotators) enough surrounding code to render
/// a useful snippet without blowing up payload size.
const CONTEXT_RADIUS: usize = 3;

/// Locate every AST match in `content` and return each as a 1-based
/// `(line, column, context_window)` tuple.
///
/// The context window is `±CONTEXT_RADIUS` lines around the match line,
/// joined with `\n`, clamped to the file's bounds. An empty vec means the
/// pattern produced no hits.
fn find_all_matches(
    content: &str,
    pattern_str: &str,
    lang_name: &str,
) -> Result<Vec<(u32, u32, String)>> {
    use ast_grep_core::matcher::Pattern;
    use ast_grep_language::{LanguageExt, SupportLang};
    use std::str::FromStr;

    let lang = SupportLang::from_str(lang_name)
        .map_err(|_| anyhow!("unknown ast-grep language: {lang_name}"))?;
    let grep = lang.ast_grep(content);
    let pattern = Pattern::try_new(pattern_str, lang)
        .map_err(|e| anyhow!("invalid ast-grep pattern `{pattern_str}`: {e:?}"))?;
    let root = grep.root();
    let mut out = Vec::new();
    for node in root.find_all(pattern) {
        // ast-grep positions are zero-based; verdicts are 1-based.
        let start = node.start_pos();
        let line = (start.line() + 1) as u32;
        let column = (start.column(&node) + 1) as u32;
        let context_str = surrounding_lines(content, line);
        out.push((line, column, context_str));
    }
    Ok(out)
}

/// Build a `±CONTEXT_RADIUS`-line window around the 1-based `line`, joined
/// with `\n`. Bounds are clamped, so a match on line 1 of a short file just
/// returns the available lines.
fn surrounding_lines(content: &str, line: u32) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let idx = (line.saturating_sub(1) as usize).min(lines.len().saturating_sub(1));
    let lo = idx.saturating_sub(CONTEXT_RADIUS);
    let hi = idx.saturating_add(CONTEXT_RADIUS + 1).min(lines.len());
    lines[lo..hi].join("\n")
}
