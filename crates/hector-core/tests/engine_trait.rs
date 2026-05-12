use hector_core::config::{Capabilities, EngineKind, Rule, Severity, WritesPolicy};
use hector_core::engine::script::ScriptEngine;
use hector_core::engine::{RuleContext, RuleEngine};
use tempfile::tempdir;

fn make_rule(script: &str) -> Rule {
    Rule {
        description: "test rule".into(),
        engine: EngineKind::Script,
        scope: vec!["*".into()],
        severity: Severity::Error,
        script: Some(script.into()),
        pattern: None,
        language: None,
        context: None,
        capabilities: Some(Capabilities {
            network: false,
            writes: WritesPolicy::None,
        }),
        fix_hint: None,
    }
}

#[test]
fn script_engine_implements_rule_engine_trait() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "clean\n").unwrap();
    let rule = make_rule("true");
    let ctx = RuleContext {
        rule_id: "noop",
        rule: &rule,
        file: &file,
        content: Some("clean\n"),
        diff: None,
        cwd: dir.path(),
        llm: None,
    };
    let engine = ScriptEngine;
    let vs = engine.run(&ctx).expect("run");
    assert!(
        vs.is_empty(),
        "passing script should produce no violation, got {vs:?}"
    );
}
