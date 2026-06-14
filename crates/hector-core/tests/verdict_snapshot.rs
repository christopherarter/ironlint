use hector_core::verdict::{Engine, Severity, Status, Verdict, Violation, SCHEMA_VERSION};

#[test]
fn engine_enum_separates_trust_from_internal() {
    let trust = serde_json::to_string(&Engine::Trust).unwrap();
    let internal = serde_json::to_string(&Engine::Internal).unwrap();
    assert_eq!(trust, "\"trust\"");
    assert_eq!(internal, "\"internal\"");
}

#[test]
fn schema_version_is_three() {
    // SCHEMA_VERSION is 3 after v3 dropped `deferred_rules` and the
    // `semantic`/`session` engine variants.
    assert_eq!(SCHEMA_VERSION, 3);
}

#[test]
fn verdict_block_serializes_to_canonical_json() {
    let v = Verdict {
        schema_version: SCHEMA_VERSION,
        hector_version: "0.1.0".to_string(),
        status: Status::Block,
        violations: vec![Violation {
            rule_id: "no-console-log".to_string(),
            severity: Severity::Error,
            engine: Engine::Script,
            file: "src/app.ts".into(),
            line: Some(42),
            column: None,
            message: "console.log not permitted in src/".to_string(),
            suggestion: None,
            context: None,
        }],
        passed_checks: vec!["no-as-any".into(), "test-coverage-on-auth".into()],
        elapsed_ms: 1340,
    };
    insta::assert_json_snapshot!(v, { ".hector_version" => "[VERSION]" });
}

#[test]
fn verdict_pass_with_no_violations() {
    let v = Verdict {
        schema_version: SCHEMA_VERSION,
        hector_version: "0.1.0".to_string(),
        status: Status::Pass,
        violations: vec![],
        passed_checks: vec!["no-console-log".into()],
        elapsed_ms: 12,
    };
    insta::assert_json_snapshot!(v, { ".hector_version" => "[VERSION]" });
}

#[test]
fn verdict_with_internal_engine_violation_serializes() {
    // Engine-runtime errors (AST refused diff, script spawn failure)
    // serialize with `engine: "internal"` and a `__internal` rule-id suffix
    // so consumers can distinguish them from real rule violations.
    let v = Verdict::from_violations(
        vec![Violation {
            rule_id: "no-derived-state__internal".to_string(),
            severity: Severity::Error,
            engine: Engine::Internal,
            file: "src/app.tsx".to_string(),
            line: None,
            column: None,
            message: "ast engine refused malformed diff".to_string(),
            suggestion: None,
            context: None,
        }],
        vec![],
        7,
    );
    // Internal engine violations resolve to InternalError, not Block.
    assert_eq!(v.status, Status::InternalError);
    insta::assert_json_snapshot!(v, { ".hector_version" => "[VERSION]" });
}

#[test]
fn two_warnings_aggregate_to_warn_status() {
    // Pin the (Warn, Warn) aggregation rule so a change to `from_violations`
    // cannot downgrade two warnings to Pass or upgrade them to Block.
    let v = Verdict::from_violations(
        vec![
            Violation {
                rule_id: "a".into(),
                severity: Severity::Warning,
                engine: Engine::Ast,
                file: "f".into(),
                line: None,
                column: None,
                message: "x".into(),
                suggestion: None,
                context: None,
            },
            Violation {
                rule_id: "b".into(),
                severity: Severity::Warning,
                engine: Engine::Ast,
                file: "f".into(),
                line: None,
                column: None,
                message: "y".into(),
                suggestion: None,
                context: None,
            },
        ],
        vec![],
        0,
    );
    assert!(matches!(v.status, Status::Warn));
}

#[test]
fn verdict_pass_constructor_returns_canonical_empty_verdict() {
    let v = Verdict::pass();
    assert_eq!(v.schema_version, SCHEMA_VERSION);
    assert_eq!(v.status, Status::Pass);
    assert!(v.violations.is_empty());
    assert!(v.passed_checks.is_empty());
    assert_eq!(v.elapsed_ms, 0);
    assert_eq!(v.hector_version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn verdict_warn_from_violations() {
    let v = Verdict::from_violations(
        vec![Violation {
            rule_id: "no-debug".to_string(),
            severity: Severity::Warning,
            engine: Engine::Ast,
            file: "src/lib.ts".to_string(),
            line: None,
            column: None,
            message: "debugger statement left in code".to_string(),
            suggestion: Some("remove debugger".to_string()),
            context: None,
        }],
        vec![],
        55,
    );
    assert_eq!(v.status, Status::Warn);
    insta::assert_json_snapshot!(v, { ".hector_version" => "[VERSION]" });
}
