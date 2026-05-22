//! H1 — runner-level test that `emit_semantic_payload: true` causes
//! `Semantic` and `Session` rules to be collected into the deferred
//! envelope rather than dispatched.

use hector_core::runner::{CheckInput, CheckOptions, HectorEngine};
use std::collections::HashSet;
use std::fs;
use tempfile::tempdir;

const CONFIG_YAML: &str = r#"
schema_version: 2
trust:
  fingerprint: PLACEHOLDER
llm:
  provider: claude-code-subagent
  model: ignored
rules:
  no-debug:
    description: no DEBUG prints in committed code
    engine: semantic
    scope: ["**/*.rs"]
    severity: error
"#;

fn write_trusted_config(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    fs::write(&path, CONFIG_YAML).unwrap();
    // Compute and rewrite the trust fingerprint so HectorEngine::load accepts it.
    let yaml = fs::read_to_string(&path).unwrap();
    let new = hector_core::trust::write_trust_block(&yaml).unwrap();
    fs::write(&path, new).unwrap();
    path
}

#[test]
fn deferred_mode_collects_semantic_rule() {
    let tmp = tempdir().unwrap();
    let config = write_trusted_config(tmp.path());
    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() { println!(\"DEBUG\"); }\n").unwrap();

    let opts = CheckOptions {
        rules: HashSet::new(),
        explain: false,
        emit_semantic_payload: true,
    };
    let engine = HectorEngine::builder()
        .with_options(opts)
        .load(&config)
        .expect("config loads with subagent provider");
    let content = fs::read_to_string(&src).unwrap();
    let report = engine
        .check_with_explain(CheckInput::File { path: src, content })
        .expect("check succeeds");

    let deferred = report
        .deferred
        .as_ref()
        .expect("deferred envelope must be present when a semantic rule is in scope");
    assert_eq!(deferred.payload.evaluate.len(), 1);
    assert_eq!(deferred.payload.evaluate[0].id, "no-debug");
    assert_eq!(deferred.payload.evaluate[0].engine, "semantic");
    // The deterministic verdict carries no semantic violations.
    assert!(
        report
            .verdict
            .violations
            .iter()
            .all(|v| v.engine != hector_core::verdict::Engine::Semantic),
        "deferred semantic rules must not produce verdict violations"
    );
    // The evaluator_input embeds the rule description verbatim.
    assert!(
        deferred
            .payload
            .evaluator_input
            .contains("no DEBUG prints in committed code"),
        "evaluator_input must include the rule description"
    );
    // File-mode check: diff is empty in the envelope.
    assert!(deferred.payload.diff.is_empty());
}

#[test]
fn deferred_mode_envelope_carries_diff_in_diff_input() {
    // Exercises the `diff.is_empty() == false` branch of
    // `build_deferred_envelope`: when the runner is given a `Diff`
    // input, the unified diff is the primary blob threaded through the
    // evaluator-input and also surfaced verbatim on the envelope's
    // `diff` field.
    let tmp = tempdir().unwrap();
    let config = write_trusted_config(tmp.path());
    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() {}\n").unwrap();

    let opts = CheckOptions {
        rules: HashSet::new(),
        explain: true,
        emit_semantic_payload: true,
    };
    let engine = HectorEngine::builder()
        .with_options(opts)
        .load(&config)
        .expect("config loads with subagent provider");

    let unified_diff = "--- a/foo.rs\n+++ b/foo.rs\n@@\n+println!(\"DEBUG\");\n".to_string();
    let report = engine
        .check_with_explain(CheckInput::Diff {
            file: src,
            unified_diff: unified_diff.clone(),
        })
        .expect("diff-mode check succeeds");

    let deferred = report.deferred.expect("deferred envelope present");
    assert_eq!(deferred.payload.diff, unified_diff);
    assert!(deferred.payload.evaluator_input.contains("DEBUG"));
    // The explain row records the deferred reason verbatim.
    assert!(report.explain.iter().any(|r| matches!(
        &r.outcome,
        hector_core::runner::ExplainOutcome::Skipped { reason } if reason == "deferred_subagent"
    )));
}
