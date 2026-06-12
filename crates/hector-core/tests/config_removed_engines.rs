use hector_core::config::parser::parse_str;

fn cfg_with_engine(engine: &str) -> String {
    format!(
        r#"
schema_version: 2
rules:
  judge-me:
    description: "llm-judged rule"
    engine: {engine}
    scope: "**/*.ts"
    severity: error
"#
    )
}

#[test]
fn semantic_rule_is_rejected_with_curated_error() {
    let err = parse_str(&cfg_with_engine("semantic"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("rule 'judge-me'"), "got: {err}");
    assert!(err.contains("engine 'semantic' was removed"), "got: {err}");
    assert!(err.contains("script or ast"), "got: {err}");
}

#[test]
fn session_rule_is_rejected_with_curated_error() {
    let err = parse_str(&cfg_with_engine("session"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("rule 'judge-me'"), "got: {err}");
    assert!(err.contains("engine 'session' was removed"), "got: {err}");
}

#[test]
fn script_and_ast_rules_still_parse() {
    let yaml = r#"
schema_version: 2
rules:
  ok-script:
    description: "fine"
    engine: script
    scope: "**/*.ts"
    severity: error
    script: "true"
"#;
    assert!(parse_str(yaml).is_ok());
}
