//! `hector show-resolved-config`. Print the post-extends merged rule
//! set in one of three formats. Read-only.

use crate::cli::ShowFormat;
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The shape that gets serialized by the YAML and JSON formatters.
///
/// Mirrors `Config` minus the two fields that are meaningless after the
/// extends merge:
/// - `trust:` — per-config-file fingerprint; the merged form has no
///   single source file to fingerprint.
/// - `extends:` — already consumed by the merge; leaving it in would
///   imply unresolved inheritance.
///
/// Constructed by [`build_view`] from a `Config` + the rule origin map.
#[derive(Debug, Serialize)]
struct ResolvedView<'a> {
    schema_version: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skip: &'a Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution: Option<&'a hector_core::config::ExecutionConfig>,
    /// Sorted-by-id rule list. Each entry carries an `origin` field
    /// alongside the rule body so the JSON shape can attribute every
    /// rule to its source file.
    rules: BTreeMap<&'a str, RuleView<'a>>,
}

#[derive(Debug, Serialize)]
struct RuleView<'a> {
    #[serde(flatten)]
    rule: &'a hector_core::config::Rule,
    origin: String,
}

fn build_view<'a>(
    cfg: &'a hector_core::config::Config,
    origins: &'a BTreeMap<String, PathBuf>,
) -> ResolvedView<'a> {
    let rules: BTreeMap<&'a str, RuleView<'a>> = cfg
        .rules
        .iter()
        .map(|(id, rule)| {
            let origin = origins
                .get(id)
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            (id.as_str(), RuleView { rule, origin })
        })
        .collect();
    ResolvedView {
        schema_version: cfg.schema_version,
        skip: &cfg.skip,
        execution: cfg.execution.as_ref(),
        rules,
    }
}

pub fn run(config: &Path, format: ShowFormat) -> Result<i32> {
    match hector_core::config::extends::resolve_with_origin(config) {
        Ok((cfg, origins)) => {
            let body = match format {
                ShowFormat::Tsv => format_tsv(&cfg, &origins),
                ShowFormat::Yaml => format_yaml(&cfg, &origins)?,
                ShowFormat::Json => format_json(&cfg, &origins)?,
            };
            print!("{body}");
            Ok(0)
        }
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            Ok(1)
        }
    }
}

fn format_tsv(
    cfg: &hector_core::config::Config,
    origins: &std::collections::BTreeMap<String, std::path::PathBuf>,
) -> String {
    let mut out = String::new();
    for (id, rule) in sorted_rules(cfg) {
        let engine = engine_kind_str(rule.engine);
        let severity = severity_str(rule.severity);
        let scope = rule.scope.join(",");
        let fix_hint = rule.fix_hint.as_deref().unwrap_or("");
        let origin = origins
            .get(id)
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        // Six columns; cell separator is a single tab; row terminator is
        // newline. Empty cells preserve column count so downstream
        // `cut -f6` still works on rows with no fix_hint.
        out.push_str(&format!(
            "{id}\t{engine}\t{severity}\t{scope}\t{fix_hint}\t{origin}\n"
        ));
    }
    out
}

/// Materialize the rule list once in deterministic id order. `BTreeMap`
/// already iterates in key order; we re-sort defensively so the output
/// contract isn't tied to the upstream container choice.
fn sorted_rules(cfg: &hector_core::config::Config) -> Vec<(&String, &hector_core::config::Rule)> {
    let mut v: Vec<(&String, &hector_core::config::Rule)> = cfg.rules.iter().collect();
    v.sort_by(|a, b| a.0.cmp(b.0));
    v
}

fn engine_kind_str(k: hector_core::config::EngineKind) -> &'static str {
    match k {
        hector_core::config::EngineKind::Script => "script",
        hector_core::config::EngineKind::Ast => "ast",
    }
}

fn severity_str(s: hector_core::config::Severity) -> &'static str {
    match s {
        hector_core::config::Severity::Error => "error",
        hector_core::config::Severity::Warning => "warning",
    }
}

fn format_yaml(
    cfg: &hector_core::config::Config,
    origins: &BTreeMap<String, PathBuf>,
) -> Result<String> {
    let view = build_view(cfg, origins);
    let body = serde_yaml::to_string(&view)?;
    Ok(annotate_yaml_with_origins(&body, origins))
}

/// Walk the rendered YAML body and inject a `# origin: <path>` comment
/// line above each rule entry. Detects rule entries by matching lines
/// of the form `^  <id>:$` *inside* the `rules:` block — every rule key
/// in `ResolvedView` is two-space-indented.
fn annotate_yaml_with_origins(body: &str, origins: &BTreeMap<String, PathBuf>) -> String {
    let mut out = String::with_capacity(body.len() + 128);
    let mut in_rules_block = false;
    for line in body.lines() {
        if line.starts_with("rules:") {
            in_rules_block = true;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_rules_block {
            // A rule-key line is `  <id>:` with exactly two leading
            // spaces and a trailing colon. Anything more deeply
            // indented is a field of the rule body, not a new rule.
            if let Some(stripped) = line.strip_prefix("  ") {
                if !stripped.starts_with(' ') && stripped.ends_with(':') && stripped.len() > 1 {
                    let id = &stripped[..stripped.len() - 1];
                    if let Some(origin) = origins.get(id) {
                        out.push_str(&format!("  # origin: {}\n", origin.display()));
                    }
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn format_json(
    cfg: &hector_core::config::Config,
    origins: &BTreeMap<String, PathBuf>,
) -> Result<String> {
    let view = build_view(cfg, origins);
    // Pretty-printed for human inspection; tooling can re-serialize.
    let body = serde_json::to_string_pretty(&view)?;
    // Trailing newline so `... | wc -l` includes the last line.
    Ok(format!("{body}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn rule_for(scope: Vec<&str>, fix_hint: Option<&str>) -> hector_core::config::Rule {
        hector_core::config::Rule {
            description: "x".into(),
            engine: hector_core::config::EngineKind::Script,
            scope: scope.into_iter().map(|s| s.to_string()).collect(),
            severity: hector_core::config::Severity::Warning,
            script: Some("true".into()),
            pattern: None,
            language: None,
            capabilities: None,
            fix_hint: fix_hint.map(|s| s.to_string()),
            output: hector_core::config::OutputMode::default(),
        }
    }

    fn cfg_with(rules: Vec<(&str, hector_core::config::Rule)>) -> hector_core::config::Config {
        let mut map = std::collections::BTreeMap::new();
        for (id, r) in rules {
            map.insert(id.to_string(), r);
        }
        hector_core::config::Config {
            schema_version: 2,
            extends: vec![],
            trust: None,
            skip: vec![],
            execution: None,
            rules: map,
        }
    }

    fn origins_for(pairs: Vec<(&str, &str)>) -> BTreeMap<String, PathBuf> {
        pairs
            .into_iter()
            .map(|(id, path)| (id.to_string(), PathBuf::from(path)))
            .collect()
    }

    #[test]
    fn tsv_emits_six_tab_separated_columns_per_row() {
        let cfg = cfg_with(vec![
            ("alpha", rule_for(vec!["*.rs", "*.txt"], Some("hint"))),
            ("zeta", rule_for(vec!["*.md"], None)),
        ]);
        let origins = origins_for(vec![
            ("alpha", "/path/to/.hector.yml"),
            ("zeta", "/path/to/parent.yml"),
        ]);
        let out = format_tsv(&cfg, &origins);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        // Sorted by id: alpha first.
        assert_eq!(
            lines[0],
            "alpha\tscript\twarning\t*.rs,*.txt\thint\t/path/to/.hector.yml"
        );
        // Empty fix_hint becomes an empty cell, not a missing column.
        assert_eq!(
            lines[1],
            "zeta\tscript\twarning\t*.md\t\t/path/to/parent.yml"
        );
    }

    #[test]
    fn yaml_strips_trust_and_extends_and_inserts_origin_comments() {
        let mut cfg = cfg_with(vec![("alpha", rule_for(vec!["*.rs"], None))]);
        // Even if a Config carries trust/extends, the view must drop them.
        cfg.trust = Some(hector_core::config::TrustBlock {
            fingerprint: "deadbeef".into(),
        });
        cfg.extends = vec!["should-not-leak.yml".into()];
        let origins = origins_for(vec![("alpha", "/path/to/.hector.yml")]);
        let out = format_yaml(&cfg, &origins).unwrap();
        assert!(!out.contains("trust:"));
        assert!(!out.contains("fingerprint:"));
        assert!(!out.contains("extends:"));
        assert!(out.contains("# origin: /path/to/.hector.yml"));
        assert!(out.contains("alpha:"));
    }

    #[test]
    fn json_serializes_rules_sorted_by_id_with_origin() {
        let cfg = cfg_with(vec![
            ("zeta", rule_for(vec!["*.md"], None)),
            ("alpha", rule_for(vec!["*.rs"], None)),
        ]);
        let origins = origins_for(vec![("alpha", "/a/.hector.yml"), ("zeta", "/a/parent.yml")]);
        let out = format_json(&cfg, &origins).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let keys: Vec<&str> = v["rules"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(keys, vec!["alpha", "zeta"]);
        assert_eq!(v["rules"]["alpha"]["origin"], "/a/.hector.yml");
        assert_eq!(v["rules"]["zeta"]["origin"], "/a/parent.yml");
        assert!(v.get("trust").is_none());
        assert!(v.get("extends").is_none());
    }

    #[test]
    fn yaml_origin_comment_only_inside_rules_block() {
        // A rule body field happens to be named the same as a rule id —
        // the annotator must not treat it as a new rule.
        let mut cfg = cfg_with(vec![("alpha", rule_for(vec!["*.rs"], None))]);
        // Force a `language:` field in the rule body to ensure the
        // annotator doesn't misinterpret deeper-indented lines.
        if let Some(r) = cfg.rules.get_mut("alpha") {
            r.language = Some("rust".into());
        }
        let origins = origins_for(vec![("alpha", "/p/.hector.yml")]);
        let out = format_yaml(&cfg, &origins).unwrap();
        // Exactly one origin comment for one rule.
        assert_eq!(out.matches("# origin:").count(), 1);
    }
}
