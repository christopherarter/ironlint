//! DeferredVerdict serde-shape lockfile. The wire format is part of the
//! adapter contract (`adapters/claude-code/hooks/hook.sh` consumes it via
//! `jq`); changing it without bumping DEFERRED_SCHEMA_VERSION is a silent
//! break.

use hector_core::verdict_deferred::{
    DeferredPayload, DeferredRule, DeferredVerdict, DEFERRED_SCHEMA_VERSION,
};

#[test]
fn deferred_schema_version_is_three() {
    // Version history:
    // - v1: initial shape.
    // - v2: added optional `payload.evaluator_model`.
    // - v3: non-additive change to `_evaluator_input` (per-call random
    //   sentinel, per-rule context expansion); `payload.warnings` is additive.
    assert_eq!(DEFERRED_SCHEMA_VERSION, 3);
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
            warnings: vec![],
        },
        elapsed_ms: 0,
    };
    insta::assert_json_snapshot!(&v, @r###"
    {
      "schema_version": 3,
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
            evaluator_input:
                "<TP-deadbeefdeadbeefdeadbeefdeadbeef>...</UE-deadbeefdeadbeefdeadbeefdeadbeef>"
                    .into(),
            evaluator_model: None,
            warnings: vec![],
        },
        elapsed_ms: 42,
    };
    insta::assert_json_snapshot!(&v);
}

/// Snapshot the payload shape with the evaluator_model override set, locking
/// the field name, position, and serialized form.
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
            evaluator_input: "<TP-...>...</UE-...>".into(),
            evaluator_model: Some("haiku".into()),
            warnings: vec![],
        },
        elapsed_ms: 0,
    };
    insta::assert_json_snapshot!(&v, @r###"
    {
      "schema_version": 3,
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
        "_evaluator_input": "<TP-...>...</UE-...>",
        "evaluator_model": "haiku"
      },
      "elapsed_ms": 0
    }
    "###);
}

/// Snapshot the payload shape with deterministic warnings carried alongside
/// the deferred rules.
#[test]
fn deferred_verdict_carries_deterministic_warnings() {
    use hector_core::verdict::Engine;
    use hector_core::verdict_deferred::DeferredWarning;
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
            evaluator_input: "<TP-...>".into(),
            evaluator_model: None,
            warnings: vec![DeferredWarning {
                rule_id: "no-todo".into(),
                engine: Engine::Script,
                file: "src/foo.rs".into(),
                line: Some(7),
                column: None,
                message: "TODO comment present".into(),
            }],
        },
        elapsed_ms: 1,
    };
    insta::assert_json_snapshot!(&v, @r###"
    {
      "schema_version": 3,
      "deferred": true,
      "hector_version": "0.2.x",
      "passed_checks": [],
      "payload": {
        "file": "src/foo.rs",
        "diff": "",
        "passed_checks": [],
        "evaluate": [],
        "_evaluator_input": "<TP-...>",
        "warnings": [
          {
            "rule_id": "no-todo",
            "engine": "script",
            "file": "src/foo.rs",
            "line": 7,
            "column": null,
            "message": "TODO comment present"
          }
        ]
      },
      "elapsed_ms": 1
    }
    "###);
}
