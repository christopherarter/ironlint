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
    let vs = engine.run(&ctx).expect("run");
    assert_eq!(vs.len(), 1, "exactly one match expected, got {vs:?}");
    let v = &vs[0];
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
    let vs = engine.run(&ctx).expect("run");
    assert!(vs.is_empty(), "no matches expected, got {vs:?}");
}

#[test]
fn ast_violation_populates_column_and_context() {
    // P1-3: AST violations must populate `column` and `context` (the verdict
    // shape defines both; AST has the data via `start_pos().column()` and we
    // can synthesize a ±N-line window around the match).
    let rule = Rule {
        description: "x".into(),
        engine: EngineKind::Ast,
        scope: vec!["**/*.rs".into()],
        severity: Severity::Warning,
        script: None,
        pattern: Some("$E.unwrap()".into()),
        language: Some("rust".into()),
        context: None,
        capabilities: None,
        fix_hint: None,
    };
    let content = "fn a() {\n    foo();\n    bar.unwrap();\n    baz();\n}\n";
    let ctx = RuleContext {
        rule_id: "no-unwrap",
        rule: &rule,
        file: std::path::Path::new("test.rs"),
        content: Some(content),
        diff: None,
        cwd: std::path::Path::new("."),
        llm: None,
    };
    let vs = AstEngine.run(&ctx).expect("run");
    assert_eq!(
        vs.len(),
        1,
        "single .unwrap() should yield exactly one violation"
    );
    let v = &vs[0];
    assert!(v.column.is_some(), "column must be populated for ast");
    let ctxstr = v
        .context
        .as_ref()
        .expect("context must be populated for ast");
    assert!(
        ctxstr.contains("foo();") && ctxstr.contains("bar.unwrap();") && ctxstr.contains("baz();"),
        "context should include surrounding ±N lines: {ctxstr}"
    );
}

// Regression: P1-11. The trait used to return `Option<Violation>`, so the
// AST engine could only ever surface the first match — every other hit in
// the same file was silently dropped. Now `RuleEngine::run` returns
// `Vec<Violation>` and AST must emit one entry per matched node.
#[test]
fn ast_returns_every_match_not_just_first() {
    let rule = Rule {
        description: "no unwrap".into(),
        engine: EngineKind::Ast,
        scope: vec!["**/*.rs".into()],
        severity: Severity::Warning,
        script: None,
        pattern: Some("$E.unwrap()".into()),
        language: Some("rust".into()),
        context: None,
        capabilities: None,
        fix_hint: None,
    };
    let content = "fn a() { x.unwrap(); y.unwrap(); z.unwrap(); }\n";
    let ctx = RuleContext {
        rule_id: "no-unwrap",
        rule: &rule,
        file: std::path::Path::new("test.rs"),
        content: Some(content),
        diff: None,
        cwd: std::path::Path::new("."),
        llm: None,
    };
    let vs = AstEngine.run(&ctx).expect("run");
    assert_eq!(
        vs.len(),
        3,
        "all three .unwrap()s must be reported, got {vs:?}"
    );
    // Distinct columns prove these are distinct match nodes, not duplicates.
    let cols: Vec<_> = vs.iter().map(|v| v.column).collect();
    assert_eq!(
        cols.iter().collect::<std::collections::HashSet<_>>().len(),
        3,
        "each match should have a distinct column: {cols:?}"
    );
}
