use hector_core::config::{EngineKind, Rule, Severity};
use hector_core::engine::ast::AstEngine;
use hector_core::engine::{RuleContext, RuleEngine};
use tempfile::tempdir;

fn make_ast_rule(pattern: &str, language: &str) -> Rule {
    Rule {
        description: "test ast rule".into(),
        engine: EngineKind::Ast,
        scope: vec!["*.ts".into()],
        severity: Severity::Error,
        script: None,
        pattern: Some(pattern.into()),
        language: Some(language.into()),
        context: None,
        capabilities: None,
        fix_hint: None,
    }
}

#[test]
fn ast_engine_matches_pattern() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.ts");
    let content = "const x = y as any;\nconst z = 5;\n";
    std::fs::write(&file, content).unwrap();
    let rule = make_ast_rule("$EXPR as any", "TypeScript");
    let ctx = RuleContext {
        rule_id: "no-as-any",
        rule: &rule,
        file: &file,
        content: Some(content),
        diff: None,
        cwd: dir.path(),
        llm: None,
    };
    let engine = AstEngine;
    let outcome = engine.run(&ctx).expect("run");
    let v = outcome.expect("violation expected");
    assert_eq!(v.rule_id, "no-as-any");
    assert_eq!(v.engine, hector_core::verdict::Engine::Ast);
    assert_eq!(v.line, Some(1));
}

#[test]
fn ast_engine_no_match_no_violation() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.ts");
    let content = "const x = 1;\n";
    std::fs::write(&file, content).unwrap();
    let rule = make_ast_rule("$EXPR as any", "TypeScript");
    let ctx = RuleContext {
        rule_id: "no-as-any",
        rule: &rule,
        file: &file,
        content: Some(content),
        diff: None,
        cwd: dir.path(),
        llm: None,
    };
    let engine = AstEngine;
    let outcome = engine.run(&ctx).expect("run");
    assert!(outcome.is_none());
}
