use hector_core::config::{parse_str, EngineKind, Severity, WritesPolicy};

const V2: &str = include_str!("../../../tests/fixtures/valid_v2.hector.yml");

#[test]
fn parses_v2_minimal() {
    let cfg = parse_str(V2).expect("parse");
    assert_eq!(cfg.schema_version, 2);

    let llm = cfg.llm.as_ref().expect("llm block");
    assert_eq!(llm.provider, "anthropic");
    assert_eq!(llm.model, "claude-sonnet-4-6");
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
