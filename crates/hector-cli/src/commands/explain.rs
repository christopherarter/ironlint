//! `hector explain <file>` — read-only scope/skip resolution report.
//!
//! Output contract:
//! * Stdout is greppable. One rule per line. A `MATCH <id> via <glob>`
//!   line for in-scope rules; a `skip <id> scope=<glob,…>` line for
//!   out-of-scope rules. Distinct casing (`MATCH` vs `skip`) lets a user
//!   `grep '^MATCH'` to filter to just the rules that fire.
//! * When the file matches a skip pattern, a leading
//!   `SKIPPED <file> via <skip-pattern>` banner is emitted on stdout
//!   *before* the per-rule rows. The per-rule rows still print so the
//!   author sees the full scope picture.
//! * Errors (missing config, untrusted config) go to stderr; exit 1.
//! * `--format json` emits a single JSON value to stdout instead of the
//!   line-oriented format. Schema below.

use crate::cli::OutputFormat;
use anyhow::Result;
use hector_core::runner::{HectorEngine, ScopeMatch, ScopeOutcomes};
use serde::Serialize;
use std::path::Path;

/// JSON shape emitted by `--format json`. Stable for tooling.
#[derive(Debug, Serialize)]
pub struct ExplainOutput {
    /// `Some({"pattern": …})` if the file matches a skip pattern.
    pub skip: Option<SkipJson>,
    /// One entry per rule, in `BTreeMap` key order.
    pub rules: Vec<ExplainEntry>,
}

#[derive(Debug, Serialize)]
pub struct SkipJson {
    pub pattern: String,
}

#[derive(Debug, Serialize)]
pub struct ExplainEntry {
    pub rule_id: String,
    /// Either `"match"` or `"skip"`. Stable string value (do not switch
    /// to a structured enum without bumping a doc'd schema version).
    pub status: String,
    /// `Some(glob)` when `status == "match"`; `None` otherwise.
    pub matched_glob: Option<String>,
    /// `Some(scopes)` when `status == "skip"`; `None` when matched.
    pub scopes: Option<Vec<String>>,
}

pub fn run(file: &Path, format: OutputFormat, config: &Path) -> Result<i32> {
    let engine = match HectorEngine::load(config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            return Ok(1);
        }
    };
    let outcomes = engine.scope_outcomes(file);
    match format {
        OutputFormat::Human => emit_human(file, &outcomes),
        OutputFormat::Json => emit_json(&outcomes)?,
    }
    Ok(0)
}

fn emit_human(file: &Path, outcomes: &ScopeOutcomes) {
    if let Some(hit) = &outcomes.skip {
        println!("SKIPPED {} via {}", file.display(), hit.pattern);
    }
    for entry in &outcomes.rules {
        match &entry.scope_match {
            ScopeMatch::Match { glob } => {
                println!("MATCH {} via {}", entry.rule_id, glob);
            }
            ScopeMatch::NoMatch { scopes } => {
                println!("skip {} scope={}", entry.rule_id, scopes.join(","));
            }
        }
    }
}

fn emit_json(outcomes: &ScopeOutcomes) -> Result<()> {
    let out = ExplainOutput {
        skip: outcomes.skip.as_ref().map(|h| SkipJson {
            pattern: h.pattern.clone(),
        }),
        rules: outcomes
            .rules
            .iter()
            .map(|r| match &r.scope_match {
                ScopeMatch::Match { glob } => ExplainEntry {
                    rule_id: r.rule_id.clone(),
                    status: "match".to_string(),
                    matched_glob: Some(glob.clone()),
                    scopes: None,
                },
                ScopeMatch::NoMatch { scopes } => ExplainEntry {
                    rule_id: r.rule_id.clone(),
                    status: "skip".to_string(),
                    matched_glob: None,
                    scopes: Some(scopes.clone()),
                },
            })
            .collect(),
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hector_core::runner::{RuleScopeEntry, ScopeOutcomes, SkipHit};

    fn match_entry(id: &str, glob: &str) -> RuleScopeEntry {
        RuleScopeEntry {
            rule_id: id.to_string(),
            engine: hector_core::config::EngineKind::Script,
            severity: hector_core::config::Severity::Warning,
            description: "d".to_string(),
            scope_match: ScopeMatch::Match {
                glob: glob.to_string(),
            },
        }
    }

    fn skip_entry(id: &str, scopes: &[&str]) -> RuleScopeEntry {
        RuleScopeEntry {
            rule_id: id.to_string(),
            engine: hector_core::config::EngineKind::Script,
            severity: hector_core::config::Severity::Warning,
            description: "d".to_string(),
            scope_match: ScopeMatch::NoMatch {
                scopes: scopes.iter().map(|s| s.to_string()).collect(),
            },
        }
    }

    #[test]
    fn json_shape_distinguishes_match_and_skip_status_strings() {
        let outcomes = ScopeOutcomes {
            skip: Some(SkipHit {
                pattern: "Cargo.lock".into(),
            }),
            rules: vec![match_entry("a", "*.md"), skip_entry("b", &["**/*.ts"])],
        };
        // Build the value the same way `emit_json` does (we can't capture
        // stdout from a unit test cleanly, so we reconstruct the JSON
        // value and assert on it).
        let out = ExplainOutput {
            skip: outcomes.skip.as_ref().map(|h| SkipJson {
                pattern: h.pattern.clone(),
            }),
            rules: outcomes
                .rules
                .iter()
                .map(|r| match &r.scope_match {
                    ScopeMatch::Match { glob } => ExplainEntry {
                        rule_id: r.rule_id.clone(),
                        status: "match".into(),
                        matched_glob: Some(glob.clone()),
                        scopes: None,
                    },
                    ScopeMatch::NoMatch { scopes } => ExplainEntry {
                        rule_id: r.rule_id.clone(),
                        status: "skip".into(),
                        matched_glob: None,
                        scopes: Some(scopes.clone()),
                    },
                })
                .collect(),
        };
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["skip"]["pattern"], "Cargo.lock");
        assert_eq!(v["rules"][0]["status"], "match");
        assert_eq!(v["rules"][0]["matched_glob"], "*.md");
        assert!(v["rules"][0]["scopes"].is_null());
        assert_eq!(v["rules"][1]["status"], "skip");
        assert!(v["rules"][1]["matched_glob"].is_null());
        assert_eq!(v["rules"][1]["scopes"][0], "**/*.ts");
    }
}
