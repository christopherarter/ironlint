//! R5 — `llm.evaluator_model` propagates through the DeferredVerdict envelope.
//!
//! Policy authors can now pin the Claude Code subagent's model from
//! `.hector.yml` instead of editing `adapters/claude-code/agents/hector-evaluator.md`
//! frontmatter (which has the "which copy of the file?" gotcha). The
//! field is free-form: Claude Code's subagent dispatch validates the
//! value at the right layer.

use hector_core::config::parse_str;
use hector_core::runner::{CheckInput, CheckOptions, HectorEngine};
use std::collections::HashSet;
use std::fs;
use tempfile::tempdir;

#[test]
fn evaluator_model_parses_off_llm_block() {
    let yaml = r#"schema_version: 2
llm:
  provider: claude-code-subagent
  evaluator_model: haiku
rules:
  r:
    description: "no DEBUG"
    engine: semantic
    scope: ["**/*.rs"]
    severity: warning
"#;
    let cfg = parse_str(yaml).expect("parses");
    let llm = cfg.llm.expect("llm block present");
    assert_eq!(llm.evaluator_model.as_deref(), Some("haiku"));
}

#[test]
fn evaluator_model_absent_deserializes_to_none() {
    // Default — no field, no override. The envelope must remain
    // byte-compatible with the pre-R5 shape.
    let yaml = r#"schema_version: 2
llm:
  provider: claude-code-subagent
rules:
  r:
    description: "no DEBUG"
    engine: semantic
    scope: ["**/*.rs"]
    severity: warning
"#;
    let cfg = parse_str(yaml).expect("parses");
    let llm = cfg.llm.expect("llm block present");
    assert!(llm.evaluator_model.is_none());
}

#[test]
fn deferred_payload_carries_evaluator_model_when_set() {
    let tmp = tempdir().unwrap();
    let yaml = r#"
schema_version: 2
trust:
  fingerprint: PLACEHOLDER
llm:
  provider: claude-code-subagent
  evaluator_model: haiku
rules:
  no-debug:
    description: "no DEBUG prints in committed code"
    engine: semantic
    scope: ["**/*.rs"]
    severity: error
"#;
    let cfg_path = tmp.path().join(".hector.yml");
    fs::write(&cfg_path, yaml).unwrap();
    let trusted =
        hector_core::trust::write_trust_block(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    fs::write(&cfg_path, trusted).unwrap();

    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() {}\n").unwrap();

    let opts = CheckOptions {
        rules: HashSet::new(),
        explain: false,
        emit_semantic_payload: true,
    };
    let engine = HectorEngine::builder()
        .with_options(opts)
        .load(&cfg_path)
        .expect("load");
    let report = engine
        .check_with_explain(CheckInput::File {
            path: src.clone(),
            content: fs::read_to_string(&src).unwrap(),
        })
        .expect("check");

    let deferred = report.deferred.expect("envelope present");
    assert_eq!(
        deferred.payload.evaluator_model.as_deref(),
        Some("haiku"),
        "evaluator_model must propagate from LlmConfig into the payload"
    );
}

#[test]
fn deferred_payload_omits_evaluator_model_when_unset() {
    // Regression: an envelope without the override must remain
    // byte-compatible with the pre-R5 shape — the field is skipped
    // entirely when None, not serialized as `"evaluator_model": null`.
    let tmp = tempdir().unwrap();
    let yaml = r#"
schema_version: 2
trust:
  fingerprint: PLACEHOLDER
llm:
  provider: claude-code-subagent
rules:
  no-debug:
    description: "no DEBUG prints in committed code"
    engine: semantic
    scope: ["**/*.rs"]
    severity: error
"#;
    let cfg_path = tmp.path().join(".hector.yml");
    fs::write(&cfg_path, yaml).unwrap();
    let trusted =
        hector_core::trust::write_trust_block(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    fs::write(&cfg_path, trusted).unwrap();

    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() {}\n").unwrap();

    let opts = CheckOptions {
        rules: HashSet::new(),
        explain: false,
        emit_semantic_payload: true,
    };
    let engine = HectorEngine::builder()
        .with_options(opts)
        .load(&cfg_path)
        .expect("load");
    let report = engine
        .check_with_explain(CheckInput::File {
            path: src.clone(),
            content: fs::read_to_string(&src).unwrap(),
        })
        .expect("check");

    let deferred = report.deferred.expect("envelope present");
    assert!(deferred.payload.evaluator_model.is_none());

    // And the JSON shape must omit the field entirely.
    let json = serde_json::to_value(&deferred).unwrap();
    assert!(
        json["payload"].get("evaluator_model").is_none(),
        "envelope without override must not carry the field; got: {json}"
    );
}
