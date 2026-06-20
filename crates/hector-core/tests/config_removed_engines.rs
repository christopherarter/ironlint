use hector_core::config::parser::parse_str;

fn cfg_with_engine(engine: &str) -> String {
    format!(
        r#"
schema_version: 2
rules:
  judge-me:
    description: "removed-engine rule"
    engine: {engine}
    scope: "**/*.ts"
    severity: error
"#
    )
}

// The `semantic`/`session` engines were removed entirely in 0.2; `EngineKind`
// only knows `script` and `ast`, so serde rejects the old values at parse with
// an `unknown variant` error. (The CLI renders the full chain via `{:#}`.)
#[test]
fn semantic_rule_is_rejected_as_unknown_variant() {
    let err = format!("{:#}", parse_str(&cfg_with_engine("semantic")).unwrap_err());
    assert!(err.contains("unknown variant"), "got: {err}");
    assert!(err.contains("semantic"), "got: {err}");
}

#[test]
fn session_rule_is_rejected_as_unknown_variant() {
    let err = format!("{:#}", parse_str(&cfg_with_engine("session")).unwrap_err());
    assert!(err.contains("unknown variant"), "got: {err}");
    assert!(err.contains("session"), "got: {err}");
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
