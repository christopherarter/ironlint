//! C2 — coverage for the read-only `scope_outcomes` helper used by
//! `hector explain` and `hector guide`. Verifies scope match reporting,
//! skip-pattern resolution, and out-of-scope listing — all without any
//! engine dispatch.

use hector_core::runner::{HectorEngine, ScopeMatch};
use std::path::PathBuf;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> PathBuf {
    let path = dir.join(".hector.yml");
    std::fs::write(&path, body).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let with_trust = hector_core::trust::write_trust_block(&raw).unwrap();
    std::fs::write(&path, with_trust).unwrap();
    path
}

const THREE_RULE_BODY: &str = "schema_version: 2\nrules:\n  ts-rule:\n    description: \"avoid foo in ts\"\n    engine: script\n    scope: [\"**/*.ts\"]\n    severity: error\n    script: \"true\"\n  rs-rule:\n    description: \"no panic in rust\"\n    engine: script\n    scope: [\"**/*.rs\"]\n    severity: warning\n    script: \"true\"\n  any-md:\n    description: \"docs lint\"\n    engine: script\n    scope: [\"*.md\"]\n    severity: warning\n    script: \"true\"\n";

#[test]
fn scope_outcomes_marks_in_scope_rules_and_lists_out_of_scope_globs() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(dir.path(), THREE_RULE_BODY);
    let engine = HectorEngine::load(&cfg).unwrap();
    let file = dir.path().join("docs/intro.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "# hi\n").unwrap();

    let outcomes = engine.scope_outcomes(&file);
    assert!(outcomes.skip.is_none(), "intro.md must not be skipped");
    assert_eq!(outcomes.rules.len(), 3, "every resolved rule appears");

    let any_md = outcomes
        .rules
        .iter()
        .find(|r| r.rule_id == "any-md")
        .expect("any-md present");
    match &any_md.scope_match {
        ScopeMatch::Match { glob } => assert_eq!(glob, "*.md"),
        other => panic!("any-md must be a Match, got {other:?}"),
    }
    let ts_rule = outcomes
        .rules
        .iter()
        .find(|r| r.rule_id == "ts-rule")
        .expect("ts-rule present");
    match &ts_rule.scope_match {
        ScopeMatch::NoMatch { scopes } => assert_eq!(scopes, &vec!["**/*.ts".to_string()]),
        other => panic!("ts-rule must be a NoMatch, got {other:?}"),
    }
}

#[test]
fn scope_outcomes_records_skip_hit_for_built_in_lockfile() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(dir.path(), THREE_RULE_BODY);
    let engine = HectorEngine::load(&cfg).unwrap();
    let lock = dir.path().join("Cargo.lock");
    std::fs::write(&lock, "# generated\n").unwrap();

    let outcomes = engine.scope_outcomes(&lock);
    let hit = outcomes.skip.expect("Cargo.lock must register a skip hit");
    assert_eq!(
        hit.pattern, "Cargo.lock",
        "the matching skip pattern surfaces verbatim"
    );
    // Per-rule rows are still produced — `explain` reports them under the
    // SKIPPED banner so the author sees the full scope picture.
    assert_eq!(outcomes.rules.len(), 3);
}

#[test]
fn scope_outcomes_returns_empty_rules_for_config_with_no_rules() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(dir.path(), "schema_version: 2\nrules: {}\n");
    let engine = HectorEngine::load(&cfg).unwrap();
    let file = dir.path().join("anything.txt");
    std::fs::write(&file, "x\n").unwrap();
    let outcomes = engine.scope_outcomes(&file);
    assert!(outcomes.skip.is_none());
    assert!(outcomes.rules.is_empty());
}
