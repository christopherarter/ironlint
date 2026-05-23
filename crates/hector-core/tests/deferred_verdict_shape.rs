//! H1 — DeferredVerdict serde-shape lockfile. The wire format is part of
//! the adapter contract (`adapters/claude-code/hooks/hook.sh` consumes it
//! via `jq`); changing it without bumping DEFERRED_SCHEMA_VERSION is a
//! silent break.

use hector_core::verdict_deferred::{
    DeferredPayload, DeferredRule, DeferredVerdict, DEFERRED_SCHEMA_VERSION,
};

#[test]
fn deferred_schema_version_is_two() {
    // R5 (2026-05-23) bumped this from 1 → 2 to reflect the new
    // optional `payload.evaluator_model` field. Old envelopes without
    // the field remain byte-compatible thanks to skip_serializing_if.
    assert_eq!(DEFERRED_SCHEMA_VERSION, 2);
}

#[test]
fn empty_deferred_verdict_serializes_to_canonical_shape() {
    let v = DeferredVerdict {
        schema_version: DEFERRED_SCHEMA_VERSION,
        deferred: true,
        hector_version: "0.2.x".to_string(),
        passed_checks: vec![],
        payload: DeferredPayload {
            file: "src/foo.rs".into(),
            diff: String::new(),
            passed_checks: vec![],
            evaluate: vec![],
            evaluator_input: String::new(),
            evaluator_model: None,
        },
        elapsed_ms: 0,
    };
    insta::assert_json_snapshot!(&v, @r###"
    {
      "schema_version": 2,
      "deferred": true,
      "hector_version": "0.2.x",
      "passed_checks": [],
      "payload": {
        "file": "src/foo.rs",
        "diff": "",
        "passed_checks": [],
        "evaluate": [],
        "_evaluator_input": ""
      },
      "elapsed_ms": 0
    }
    "###);
}

#[test]
fn deferred_verdict_with_two_rules_serializes() {
    let v = DeferredVerdict {
        schema_version: DEFERRED_SCHEMA_VERSION,
        deferred: true,
        hector_version: "0.2.x".to_string(),
        passed_checks: vec!["det-1".into(), "det-2".into()],
        payload: DeferredPayload {
            file: "src/foo.rs".into(),
            diff: "@@ -1,1 +1,1 @@\n-old\n+new\n".into(),
            passed_checks: vec!["det-1".into(), "det-2".into()],
            evaluate: vec![
                DeferredRule {
                    id: "no-debug".into(),
                    description: "no DEBUG prints".into(),
                    severity: "error".into(),
                    engine: "semantic".into(),
                },
                DeferredRule {
                    id: "schema-needs-migration".into(),
                    description: "schema edits require a migration file".into(),
                    severity: "warning".into(),
                    engine: "session".into(),
                },
            ],
            evaluator_input: "<TRUSTED_POLICY>...</UNTRUSTED_EVIDENCE>".into(),
            evaluator_model: None,
        },
        elapsed_ms: 42,
    };
    insta::assert_json_snapshot!(&v);
}

/// R5: snapshot the payload shape WITH the evaluator_model override set.
/// Locks the field name, position, and serialized form.
#[test]
fn deferred_verdict_carries_evaluator_model_when_set() {
    let v = DeferredVerdict {
        schema_version: DEFERRED_SCHEMA_VERSION,
        deferred: true,
        hector_version: "0.2.x".to_string(),
        passed_checks: vec![],
        payload: DeferredPayload {
            file: "src/foo.rs".into(),
            diff: String::new(),
            passed_checks: vec![],
            evaluate: vec![DeferredRule {
                id: "no-debug".into(),
                description: "no DEBUG prints".into(),
                severity: "error".into(),
                engine: "semantic".into(),
            }],
            evaluator_input: "<TRUSTED_POLICY>...</UNTRUSTED_EVIDENCE>".into(),
            evaluator_model: Some("haiku".into()),
        },
        elapsed_ms: 0,
    };
    insta::assert_json_snapshot!(&v, @r###"
    {
      "schema_version": 2,
      "deferred": true,
      "hector_version": "0.2.x",
      "passed_checks": [],
      "payload": {
        "file": "src/foo.rs",
        "diff": "",
        "passed_checks": [],
        "evaluate": [
          {
            "id": "no-debug",
            "description": "no DEBUG prints",
            "severity": "error",
            "engine": "semantic"
          }
        ],
        "_evaluator_input": "<TRUSTED_POLICY>...</UNTRUSTED_EVIDENCE>",
        "evaluator_model": "haiku"
      },
      "elapsed_ms": 0
    }
    "###);
}
