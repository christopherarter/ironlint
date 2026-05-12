//! AST engine implementation via the `ast_grep_core` library.
//!
//! Uses `ast-grep-core` + `ast-grep-language` crates (v0.42) to perform
//! structural pattern matching directly in-process (no CLI subprocess).

use crate::engine::{RuleContext, RuleEngine};
use crate::verdict::{Engine, Severity, Violation};
use anyhow::{anyhow, Result};

pub struct AstEngine;

impl RuleEngine for AstEngine {
    fn run(&self, ctx: &RuleContext) -> Result<Option<Violation>> {
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

        let first_match_line = find_first_match(content, pattern_str, lang_name)?;
        let Some(line) = first_match_line else {
            return Ok(None);
        };

        let severity = match ctx.rule.severity {
            crate::config::Severity::Error => Severity::Error,
            crate::config::Severity::Warning => Severity::Warning,
        };
        Ok(Some(Violation {
            rule_id: ctx.rule_id.to_string(),
            severity,
            engine: Engine::Ast,
            file: ctx.file.display().to_string(),
            line: Some(line),
            column: None,
            message: ctx.rule.description.clone(),
            suggestion: ctx.rule.fix_hint.clone(),
            context: None,
        }))
    }
}

fn find_first_match(content: &str, pattern_str: &str, lang_name: &str) -> Result<Option<u32>> {
    use ast_grep_core::matcher::Pattern;
    use ast_grep_language::{LanguageExt, SupportLang};
    use std::str::FromStr;

    let lang = SupportLang::from_str(lang_name)
        .map_err(|_| anyhow!("unknown ast-grep language: {lang_name}"))?;
    let grep = lang.ast_grep(content);
    let pattern = Pattern::try_new(pattern_str, lang)
        .map_err(|e| anyhow!("invalid ast-grep pattern `{pattern_str}`: {e:?}"))?;
    // start_pos().line() is zero-based; convert to 1-based.
    let line = grep
        .root()
        .find_all(pattern)
        .next()
        .map(|node| (node.start_pos().line() + 1) as u32);
    Ok(line)
}
