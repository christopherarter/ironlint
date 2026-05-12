use anyhow::Result;
use hector_core::config::{ContextScope, EngineKind, Rule, Severity};
use hector_core::engine::semantic::SemanticEngine;
use hector_core::engine::{RuleContext, RuleEngine};
use hector_core::llm::{LlmClient, RuleStatus, RuleVerdict};
use tempfile::tempdir;

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
    std::fs::write(
        &file,
        "const X = () => useEffect(() => setY(props.y), [props.y]);\n",
    )
    .unwrap();
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

// Regression: P1-6. Bug-audit finding — when the LLM hallucinates a different
// rule_id than the one we requested, the engine used to silently return
// Ok(None) (a "pass" verdict for a rule we never actually got an answer for).
// The engine must instead bail so the runner surfaces it as an internal
// engine error.
#[test]
fn semantic_engine_errors_on_rule_id_mismatch() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("app.tsx");
    std::fs::write(&file, "const X = () => null;\n").unwrap();
    // LLM returns a verdict for a hallucinated rule_id, not the one queried.
    let llm = FakeLlm {
        canned: vec![RuleVerdict {
            rule_id: "hallucinated".to_string(),
            status: RuleStatus::Pass,
        }],
    };
    let rule = make_semantic_rule(ContextScope::File);
    let ctx = RuleContext {
        rule_id: "expected-id",
        rule: &rule,
        file: &file,
        content: Some(""),
        diff: None,
        cwd: dir.path(),
        llm: Some(&llm),
    };
    let engine = SemanticEngine;
    let err = engine
        .run(&ctx)
        .expect_err("expected engine to bail on rule_id mismatch");
    let chain = format!("{err:#}");
    assert!(
        chain.contains("expected-id"),
        "error must mention the requested rule_id; got: {chain}"
    );
}
