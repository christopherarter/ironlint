//! Runner-level coverage for CheckOptions: rule-id filter, explain capture.

use hector_core::runner::{CheckInput, CheckOptions, HectorEngine};
use std::collections::HashSet;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    std::fs::write(&path, body).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let with_trust = hector_core::trust::write_trust_block(&raw).unwrap();
    std::fs::write(&path, with_trust).unwrap();
    path
}

#[test]
fn explain_captures_every_in_scope_rule() {
    let dir = tempdir().unwrap();
    let body = "schema_version: 2\nrules:\n  pass-rule:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n  fire-rule:\n    description: \"y\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"exit 1\"\n  out-of-scope:\n    description: \"z\"\n    engine: script\n    scope: [\"*.rs\"]\n    severity: error\n    script: \"true\"\n";
    let cfg = write_trusted(dir.path(), body);
    let file = dir.path().join("foo.txt");
    std::fs::write(&file, "x\n").unwrap();

    let opts = CheckOptions {
        explain: true,
        ..CheckOptions::default()
    };
    let engine = HectorEngine::builder()
        .with_options(opts)
        .load(&cfg)
        .unwrap();
    let report = engine
        .check_with_explain(CheckInput::File {
            path: file.clone(),
            content: "x\n".to_string(),
        })
        .unwrap();
    assert_eq!(
        report.explain.len(),
        2,
        "only in-scope rules appear: {:?}",
        report.explain
    );
    let ids: Vec<&str> = report.explain.iter().map(|e| e.rule_id.as_str()).collect();
    assert!(ids.contains(&"pass-rule"));
    assert!(ids.contains(&"fire-rule"));
    assert!(!ids.contains(&"out-of-scope"));

    let fire = report
        .explain
        .iter()
        .find(|e| e.rule_id == "fire-rule")
        .unwrap();
    assert!(matches!(
        fire.outcome,
        hector_core::runner::ExplainOutcome::Fire
    ));
    let pass = report
        .explain
        .iter()
        .find(|e| e.rule_id == "pass-rule")
        .unwrap();
    assert!(matches!(
        pass.outcome,
        hector_core::runner::ExplainOutcome::Pass
    ));
}

#[test]
fn explain_off_leaves_explain_vec_empty() {
    let dir = tempdir().unwrap();
    let body = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n";
    let cfg = write_trusted(dir.path(), body);
    let file = dir.path().join("foo.txt");
    std::fs::write(&file, "x\n").unwrap();
    // Default options → explain=false → vec must be empty.
    let engine = HectorEngine::load(&cfg).unwrap();
    let report = engine
        .check_with_explain(CheckInput::File {
            path: file.clone(),
            content: "x\n".to_string(),
        })
        .unwrap();
    assert!(report.explain.is_empty());
    assert!(report.verdict.passed_checks.iter().any(|id| id == "r"));
}

#[test]
fn rule_filter_runs_only_listed_ids() {
    let dir = tempdir().unwrap();
    let body = "schema_version: 2\nrules:\n  keep:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n  drop:\n    description: \"y\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"exit 1\"\n";
    let cfg = write_trusted(dir.path(), body);
    let file = dir.path().join("ok.txt");
    std::fs::write(&file, "clean\n").unwrap();

    let mut keep: HashSet<String> = HashSet::new();
    keep.insert("keep".to_string());
    let opts = CheckOptions {
        rules: keep,
        ..CheckOptions::default()
    };
    let engine = HectorEngine::builder()
        .with_options(opts)
        .load(&cfg)
        .unwrap();
    let verdict = engine
        .check(CheckInput::File {
            path: file.clone(),
            content: "clean\n".to_string(),
        })
        .unwrap();

    assert!(verdict.passed_checks.iter().any(|id| id == "keep"));
    assert!(!verdict.passed_checks.iter().any(|id| id == "drop"));
    assert!(verdict.violations.is_empty());
}
