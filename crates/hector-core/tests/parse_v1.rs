use hector_core::config::parse_str;

const V1: &str = include_str!("../../../tests/fixtures/valid_v1.bully.yml");

#[test]
fn parses_v1_with_deprecation_flag() {
    let cfg = parse_str(V1).expect("parse v1");
    assert_eq!(cfg.schema_version, 1);
    let r = cfg.rules.get("ruff-check").expect("rule");
    assert_eq!(r.scope, vec!["*.py"]);
    assert_eq!(r.script.as_deref(), Some("ruff check --quiet {file}"));
}

#[test]
fn rejects_unknown_schema_version() {
    let bad = "schema_version: 99\nrules: {}\n";
    let result = parse_str(bad);
    assert!(result.is_err());
    let err = format!("{:#}", result.unwrap_err());
    assert!(
        err.contains("schema_version"),
        "error mentions schema_version: {err}"
    );
}
