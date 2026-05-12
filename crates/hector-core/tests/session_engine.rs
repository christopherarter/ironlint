use anyhow::Result;
use hector_core::config::{ContextScope, EngineKind, Rule, Severity};
use hector_core::engine::session::SessionEngine;
use hector_core::llm::{LlmClient, RuleStatus, RuleVerdict};
use hector_core::session_state::{EditRecord, SessionState};
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

fn make_session_rule() -> Rule {
    Rule {
        description: "Auth changes need test changes in the same session".into(),
        engine: EngineKind::Session,
        scope: vec!["src/auth/**".into()],
        severity: Severity::Error,
        script: None,
        pattern: None,
        language: None,
        context: Some(ContextScope::Repo),
        capabilities: None,
        fix_hint: None,
    }
}

#[test]
fn session_engine_evaluates_aggregated_diff() {
    let _dir = tempdir().unwrap();
    let state = SessionState {
        session_id: "s1".into(),
        started_at: "t".into(),
        edits: vec![
            EditRecord {
                file: "src/auth/login.ts".into(),
                diff: "+ change".into(),
                timestamp: "t".into(),
            },
            EditRecord {
                file: "src/auth/session.ts".into(),
                diff: "+ another".into(),
                timestamp: "t2".into(),
            },
        ],
    };
    let llm = FakeLlm {
        canned: vec![RuleVerdict {
            rule_id: "audit-tests".into(),
            status: RuleStatus::Violation {
                message: "auth changed but no test files in session".into(),
                line: None,
            },
        }],
    };
    let rule = make_session_rule();
    let engine = SessionEngine;
    let v = engine
        .evaluate(&state, "audit-tests", &rule, &llm)
        .expect("evaluate")
        .expect("violation");
    assert_eq!(v.rule_id, "audit-tests");
    assert!(v.message.contains("auth changed"));
    assert_eq!(v.engine, hector_core::verdict::Engine::Session);
}

#[test]
fn session_engine_returns_none_on_llm_pass() {
    let state = SessionState {
        session_id: "s1".into(),
        started_at: "t".into(),
        edits: vec![EditRecord {
            file: "src/auth/login.ts".into(),
            diff: "+ change".into(),
            timestamp: "t".into(),
        }],
    };
    let llm = FakeLlm {
        canned: vec![RuleVerdict {
            rule_id: "audit-tests".into(),
            status: RuleStatus::Pass,
        }],
    };
    let rule = make_session_rule();
    let engine = SessionEngine;
    let result = engine
        .evaluate(&state, "audit-tests", &rule, &llm)
        .expect("evaluate");
    assert!(
        result.is_none(),
        "expected None when LLM reports Pass, got: {result:?}"
    );
}

#[test]
fn session_engine_returns_none_when_llm_omits_rule() {
    let state = SessionState {
        session_id: "s1".into(),
        started_at: "t".into(),
        edits: vec![EditRecord {
            file: "src/auth/login.ts".into(),
            diff: "+ change".into(),
            timestamp: "t".into(),
        }],
    };
    // LLM returns a verdict for a different rule than the one being queried.
    let llm = FakeLlm {
        canned: vec![RuleVerdict {
            rule_id: "some-other-rule".into(),
            status: RuleStatus::Violation {
                message: "shouldn't apply".into(),
                line: None,
            },
        }],
    };
    let rule = make_session_rule();
    let engine = SessionEngine;
    let result = engine
        .evaluate(&state, "audit-tests", &rule, &llm)
        .expect("evaluate");
    assert!(
        result.is_none(),
        "expected None when LLM omits the queried rule_id, got: {result:?}"
    );
}
