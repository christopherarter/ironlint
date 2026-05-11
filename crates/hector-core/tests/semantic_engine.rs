use hector_core::config::{ContextScope, EngineKind, Rule, Severity};
use hector_core::engine::semantic::SemanticEngine;
use hector_core::engine::{RuleContext, RuleEngine};
use hector_core::llm::{LlmClient, RuleStatus, RuleVerdict};
use tempfile::tempdir;
use anyhow::Result;

struct FakeLlm {
    canned: Vec<RuleVerdict>,
}

impl LlmClient for FakeLlm {
    fn evaluate(
        &self,
        _rules: &[(&str, &Rule)],
        _primary: &str,
        _context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>> {
        Ok(self.canned.clone())
    }
}

fn make_semantic_rule(ctx: ContextScope) -> Rule {
    Rule {
        description: "no useEffect deriving state from props".into(),
        engine: EngineKind::Semantic,
        scope: vec!["*.tsx".into()],
        severity: Severity::Warning,
        script: None,
        pattern: None,
        language: None,
        context: Some(ctx),
        capabilities: None,
        fix_hint: None,
    }
}

#[test]
fn semantic_engine_returns_violation_when_llm_says_so() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("app.tsx");
    std::fs::write(&file, "const X = () => useEffect(() => setY(props.y), [props.y]);\n").unwrap();
    let llm = FakeLlm {
        canned: vec![RuleVerdict {
            rule_id: "no-derived-state".to_string(),
            status: RuleStatus::Violation {
                message: "useEffect derives state".into(),
                line: Some(1),
            },
        }],
    };
    let rule = make_semantic_rule(ContextScope::File);
    let ctx = RuleContext {
        rule_id: "no-derived-state",
        rule: &rule,
        file: &file,
        content: Some(""),
        diff: None,
        cwd: dir.path(),
        llm: Some(&llm),
    };
    let engine = SemanticEngine;
    let v = engine.run(&ctx).expect("run").expect("violation");
    assert_eq!(v.rule_id, "no-derived-state");
    assert_eq!(v.line, Some(1));
    assert_eq!(v.engine, hector_core::verdict::Engine::Semantic);
}

#[test]
fn semantic_engine_returns_none_when_llm_says_pass() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("app.tsx");
    std::fs::write(&file, "const X = () => null;\n").unwrap();
    let llm = FakeLlm {
        canned: vec![RuleVerdict {
            rule_id: "no-derived-state".to_string(),
            status: RuleStatus::Pass,
        }],
    };
    let rule = make_semantic_rule(ContextScope::File);
    let ctx = RuleContext {
        rule_id: "no-derived-state",
        rule: &rule,
        file: &file,
        content: Some(""),
        diff: None,
        cwd: dir.path(),
        llm: Some(&llm),
    };
    let engine = SemanticEngine;
    let outcome = engine.run(&ctx).expect("run");
    assert!(outcome.is_none());
}
