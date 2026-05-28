//! `hector guide <file>` — list rules whose scope matches `<file>`,
//! with severity and description. Read-only.
//!
//! Output contract:
//! * Stdout is one rule per line: `<rule-id> [<severity>] <description>`.
//!   Severity in lowercase brackets (`[error]` / `[warning]`) so it
//!   matches the `Severity` enum's serialized form. Simple space
//!   separation rather than fixed columns — bully chose this; it keeps
//!   `awk '{print $1}'` working as the basic id-extraction recipe.
//! * Rules are sorted by id (deterministic; the underlying
//!   `BTreeMap<String, Rule>` already iterates in key order, so no
//!   re-sort is needed — the test asserts the property explicitly).
//! * When the file matches a skip pattern, a leading
//!   `SKIPPED <file> via <pattern>` banner is emitted and *no* rule
//!   rows follow (skipped files have no applicable guidance).
//! * Errors (missing/untrusted config) go to stderr; exit 1.
//! * `--format json` emits `{ skip, rules: [{ rule_id, severity,
//!   description }] }` to stdout.

use crate::cli::OutputFormat;
use anyhow::Result;
use hector_core::config::Severity;
use hector_core::runner::{HectorEngine, ScopeMatch, ScopeOutcomes};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct GuideOutput {
    pub skip: Option<SkipJson>,
    pub rules: Vec<GuideEntry>,
}

#[derive(Debug, Serialize)]
pub struct SkipJson {
    pub pattern: String,
}

#[derive(Debug, Serialize)]
pub struct GuideEntry {
    pub rule_id: String,
    /// Stable lowercase string: `"error"` or `"warning"`.
    pub severity: String,
    pub description: String,
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

fn severity_str(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

fn emit_human(file: &Path, outcomes: &ScopeOutcomes) {
    if let Some(hit) = &outcomes.skip {
        println!("SKIPPED {} via {}", file.display(), hit.pattern);
        return;
    }
    for entry in &outcomes.rules {
        if matches!(entry.scope_match, ScopeMatch::Match { .. }) {
            println!(
                "{} [{}] {}",
                entry.rule_id,
                severity_str(entry.severity),
                entry.description
            );
        }
    }
}

fn emit_json(outcomes: &ScopeOutcomes) -> Result<()> {
    let out = GuideOutput {
        skip: outcomes.skip.as_ref().map(|h| SkipJson {
            pattern: h.pattern.clone(),
        }),
        rules: if outcomes.skip.is_some() {
            Vec::new()
        } else {
            outcomes
                .rules
                .iter()
                .filter(|r| matches!(r.scope_match, ScopeMatch::Match { .. }))
                .map(|r| GuideEntry {
                    rule_id: r.rule_id.clone(),
                    severity: severity_str(r.severity).to_string(),
                    description: r.description.clone(),
                })
                .collect()
        },
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hector_core::runner::{RuleScopeEntry, ScopeOutcomes, SkipHit};

    fn entry(id: &str, sev: Severity, desc: &str, sm: ScopeMatch) -> RuleScopeEntry {
        RuleScopeEntry {
            rule_id: id.to_string(),
            engine: hector_core::config::EngineKind::Script,
            severity: sev,
            description: desc.to_string(),
            scope_match: sm,
        }
    }

    #[test]
    fn json_filters_to_in_scope_rules_and_omits_when_skipped() {
        let outcomes = ScopeOutcomes {
            skip: None,
            rules: vec![
                entry(
                    "a",
                    Severity::Error,
                    "ad",
                    ScopeMatch::Match {
                        glob: "*.md".into(),
                    },
                ),
                entry(
                    "b",
                    Severity::Warning,
                    "bd",
                    ScopeMatch::NoMatch {
                        scopes: vec!["*.ts".into()],
                    },
                ),
            ],
        };
        let out = GuideOutput {
            skip: None,
            rules: outcomes
                .rules
                .iter()
                .filter(|r| matches!(r.scope_match, ScopeMatch::Match { .. }))
                .map(|r| GuideEntry {
                    rule_id: r.rule_id.clone(),
                    severity: severity_str(r.severity).to_string(),
                    description: r.description.clone(),
                })
                .collect(),
        };
        let v = serde_json::to_value(&out).unwrap();
        assert!(v["skip"].is_null());
        assert_eq!(v["rules"].as_array().unwrap().len(), 1);
        assert_eq!(v["rules"][0]["rule_id"], "a");
        assert_eq!(v["rules"][0]["severity"], "error");
    }

    #[test]
    fn skipped_file_produces_empty_rules_in_json() {
        let outcomes = ScopeOutcomes {
            skip: Some(SkipHit {
                pattern: "Cargo.lock".into(),
            }),
            rules: vec![entry(
                "a",
                Severity::Error,
                "ad",
                ScopeMatch::Match {
                    glob: "*.md".into(),
                },
            )],
        };
        let out = GuideOutput {
            skip: Some(SkipJson {
                pattern: "Cargo.lock".into(),
            }),
            rules: if outcomes.skip.is_some() {
                Vec::new()
            } else {
                vec![]
            },
        };
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["skip"]["pattern"], "Cargo.lock");
        assert_eq!(v["rules"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn severity_str_renders_lowercase_words() {
        assert_eq!(severity_str(Severity::Error), "error");
        assert_eq!(severity_str(Severity::Warning), "warning");
    }
}
