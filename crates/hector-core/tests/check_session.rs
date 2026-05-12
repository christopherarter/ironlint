use anyhow::Result;
use hector_core::config::Rule;
use hector_core::llm::{LlmClient, RuleStatus, RuleVerdict};
use hector_core::runner::HectorEngine;
use hector_core::session_state::{EditRecord, SessionState};
use hector_core::verdict::{Engine, Status};
use std::fs;
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

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    fs::write(&path, body).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    let with_trust = hector_core::trust::write_trust_block(&raw).unwrap();
    fs::write(&path, with_trust).unwrap();
    path
}

const SESSION_RULE_CONFIG: &str = "schema_version: 2\nrules:\n  audit-tests:\n    description: \"auth changes need tests\"\n    engine: session\n    scope: [\"src/auth/**\"]\n    severity: error\n    context: repo\n";

fn session_with_one_edit() -> SessionState {
    SessionState {
        session_id: "s1".into(),
        started_at: "t".into(),
        edits: vec![EditRecord {
            file: "src/auth/login.ts".into(),
            diff: "+ const x = 1;".into(),
            timestamp: "t".into(),
        }],
    }
}

#[test]
fn check_session_returns_violation_when_llm_says_so() {
    let dir = tempdir().unwrap();
    let path = write_trusted(dir.path(), SESSION_RULE_CONFIG);
    let fake = FakeLlm {
        canned: vec![RuleVerdict {
            rule_id: "audit-tests".into(),
            status: RuleStatus::Violation {
                message: "auth changed but no test files in session".into(),
                line: None,
            },
        }],
    };
    let engine = HectorEngine::builder()
        .with_llm(Box::new(fake))
        .load(&path)
        .expect("load");

    let state = session_with_one_edit();
    let verdict = engine.check_session(&state).expect("check_session");
    assert_eq!(verdict.status, Status::Block);
    assert_eq!(verdict.violations.len(), 1);
    let v = &verdict.violations[0];
    assert_eq!(v.rule_id, "audit-tests");
    assert_eq!(v.message, "auth changed but no test files in session");
    assert_eq!(v.engine, Engine::Session);
}

#[test]
fn session_rule_with_scope_no_matching_edits_passes_trivially() {
    // P2-17: a session rule scoped to `src/auth/**` must NOT fire when
    // every recorded edit lives outside that scope. The aggregation step
    // should filter edits by scope before invoking the LLM; if the
    // filtered list is empty the rule trivially passes and the LLM is
    // never asked.
    let dir = tempdir().unwrap();
    let path = write_trusted(dir.path(), SESSION_RULE_CONFIG);
    // The fake is canned to "Violation" — if the runner forwards to it,
    // the test fails. The only way to reach Pass is to short-circuit
    // before the LLM call.
    let fake = FakeLlm {
        canned: vec![RuleVerdict {
            rule_id: "audit-tests".into(),
            status: RuleStatus::Violation {
                message: "should not have been called".into(),
                line: None,
            },
        }],
    };
    let engine = HectorEngine::builder()
        .with_llm(Box::new(fake))
        .load(&path)
        .expect("load");

    let state = SessionState {
        session_id: "s1".into(),
        started_at: "t".into(),
        edits: vec![EditRecord {
            file: "src/billing/checkout.ts".into(),
            diff: "+ const x = 1;".into(),
            timestamp: "t".into(),
        }],
    };
    let verdict = engine.check_session(&state).expect("check_session");
    assert_eq!(verdict.status, Status::Pass);
    assert!(
        verdict.violations.is_empty(),
        "rule must not fire when no edits match scope"
    );
    assert!(verdict.passed_checks.iter().any(|r| r == "audit-tests"));
}

#[test]
fn check_session_returns_pass_when_llm_says_pass() {
    let dir = tempdir().unwrap();
    let path = write_trusted(dir.path(), SESSION_RULE_CONFIG);
    let fake = FakeLlm {
        canned: vec![RuleVerdict {
            rule_id: "audit-tests".into(),
            status: RuleStatus::Pass,
        }],
    };
    let engine = HectorEngine::builder()
        .with_llm(Box::new(fake))
        .load(&path)
        .expect("load");

    let state = session_with_one_edit();
    let verdict = engine.check_session(&state).expect("check_session");
    assert_eq!(verdict.status, Status::Pass);
    assert!(verdict.violations.is_empty());
    assert!(verdict.passed_checks.iter().any(|r| r == "audit-tests"));
}
