use hector_core::config::{parse_str, EngineKind, Severity, WritesPolicy};

const V2: &str = include_str!("../../../tests/fixtures/valid_v2.hector.yml");

#[test]
fn parses_v2_minimal() {
    let cfg = parse_str(V2).expect("parse");
    assert_eq!(cfg.schema_version, 2);

    let llm = cfg.llm.as_ref().expect("llm block");
    assert_eq!(llm.provider, "anthropic");
    assert_eq!(llm.model.as_deref(), Some("claude-sonnet-4-6"));
    assert_eq!(llm.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));

    let r = cfg.rules.get("no-console-log").expect("rule present");
    assert_eq!(r.engine, EngineKind::Script);
    assert_eq!(r.severity, Severity::Error);
    assert_eq!(r.scope, vec!["src/**/*.ts"]);
    assert_eq!(
        r.script.as_deref(),
        Some("grep -nE 'console\\.log\\(' {file} && exit 1 || exit 0")
    );
    let caps = r.capabilities.as_ref().expect("caps");
    assert!(!caps.network);
    assert_eq!(caps.writes, WritesPolicy::CwdOnly);

    let ast = cfg.rules.get("no-as-any").expect("ast rule");
    assert_eq!(ast.engine, EngineKind::Ast);
    assert_eq!(ast.pattern.as_deref(), Some("$EXPR as any"));
    assert_eq!(ast.language.as_deref(), Some("ts"));
    assert_eq!(ast.scope, vec!["src/**/*.ts"]); // single-string scope normalized to vec
}

#[test]
fn parses_execution_max_workers() {
    let yaml = r#"schema_version: 2
execution:
  max_workers: 4
rules:
  r1:
    description: "no foo"
    engine: ast
    scope: ["**/*.rs"]
    severity: warning
    pattern: "$X"
    language: rust
"#;
    let cfg = hector_core::config::parse_str(yaml).expect("parse");
    let exec = cfg.execution.as_ref().expect("execution block");
    assert_eq!(exec.max_workers, 4);
}

#[test]
fn capabilities_writes_defaults_to_none_when_omitted() {
    let yaml = r#"schema_version: 2
rules:
  r:
    description: "x"
    engine: script
    scope: ["*"]
    severity: error
    script: "true"
    capabilities:
      network: false
"#;
    let cfg = hector_core::config::parse_str(yaml).expect("parse");
    let caps = cfg.rules["r"].capabilities.as_ref().unwrap();
    assert_eq!(caps.writes, WritesPolicy::None);
}

#[test]
fn scope_rejects_non_string_non_sequence_values() {
    let yaml = r#"schema_version: 2
rules:
  r:
    description: "x"
    engine: script
    scope: 42
    severity: error
    script: "true"
"#;
    let err = hector_core::config::parse_str(yaml).expect_err("scope must be string or list");
    assert!(format!("{err:#}").contains("scope"));
}

#[test]
fn scope_rejects_sequence_entries_that_are_not_strings() {
    let yaml = r#"schema_version: 2
rules:
  r:
    description: "x"
    engine: script
    scope: [1, 2]
    severity: error
    script: "true"
"#;
    let err = hector_core::config::parse_str(yaml).expect_err("entries must be strings");
    assert!(format!("{err:#}").contains("string"));
}

#[test]
fn parses_without_execution_block() {
    let yaml = r#"schema_version: 2
rules:
  r1:
    description: "no foo"
    engine: ast
    scope: ["**/*.rs"]
    severity: warning
    pattern: "$X"
    language: rust
"#;
    let cfg = hector_core::config::parse_str(yaml).expect("parse");
    assert!(cfg.execution.is_none());
}
