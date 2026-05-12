use hector_core::verdict::{Engine, Severity, Status, Verdict, Violation};

#[test]
fn verdict_block_serializes_to_canonical_json() {
    let v = Verdict {
        schema_version: 1,
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
        schema_version: 1,
        hector_version: "0.1.0".to_string(),
        status: Status::Pass,
        violations: vec![],
        passed_checks: vec!["no-console-log".into()],
        elapsed_ms: 12,
    };
    insta::assert_json_snapshot!(v, { ".hector_version" => "[VERSION]" });
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
