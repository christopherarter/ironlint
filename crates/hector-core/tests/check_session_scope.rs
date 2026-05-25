//! B2 regression: session-engine rules with pathed scopes must match
//! when SessionState.edits carry absolute paths (the adapter shape).

use anyhow::Result;
use hector_core::config::Rule;
use hector_core::llm::{LlmClient, RuleVerdict};
use hector_core::runner::HectorEngine;
use hector_core::session_state::{EditRecord, SessionState};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tempfile::tempdir;

const CFG: &str = r#"
schema_version: 2
rules:
  auth-changes-need-review:
    description: any change under src/auth requires manual review
    engine: session
    scope: ["src/auth/**"]
    severity: error
"#;

#[derive(Default)]
struct FakeLlm {
    called: Arc<AtomicBool>,
}

impl LlmClient for FakeLlm {
    fn evaluate(
        &self,
        _rules: &[(&str, &Rule)],
        _primary: &str,
        _context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>> {
        self.called.store(true, Ordering::SeqCst);
        Ok(vec![])
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

#[test]
fn session_rule_matches_absolute_path_for_pathed_scope() {
    let tmp = tempdir().unwrap();
    let cfg_path = write_trusted(tmp.path(), CFG);

    let llm = FakeLlm::default();
    let llm_called = llm.called.clone();
    let engine = HectorEngine::builder()
        .with_llm(Box::new(llm))
        .load(&cfg_path)
        .expect("load");

    // Absolute path under config_dir — must match `src/auth/**`.
    let abs_under = tmp.path().join("src/auth/login.ts");
    fs::create_dir_all(abs_under.parent().unwrap()).unwrap();
    fs::write(&abs_under, "x").unwrap();

    let state = SessionState {
        session_id: "s".into(),
        started_at: "2026-05-25T00:00:00Z".into(),
        edits: vec![EditRecord {
            file: abs_under.to_string_lossy().into_owned(),
            diff: "+ const x = 1;".into(),
            timestamp: "2026-05-25T00:00:01Z".into(),
        }],
    };

    let _ = engine.check_session(&state);
    assert!(
        llm_called.load(Ordering::SeqCst),
        "session LLM must be called when pathed scope matches the absolute edit path"
    );
}

#[test]
fn session_rule_does_not_match_unrelated_path() {
    let tmp = tempdir().unwrap();
    let cfg_path = write_trusted(tmp.path(), CFG);

    let llm = FakeLlm::default();
    let llm_called = llm.called.clone();
    let engine = HectorEngine::builder()
        .with_llm(Box::new(llm))
        .load(&cfg_path)
        .expect("load");

    // Edit under src/billing — must NOT match src/auth/**.
    let other = tmp.path().join("src/billing/charge.ts");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::write(&other, "x").unwrap();

    let state = SessionState {
        session_id: "s".into(),
        started_at: "2026-05-25T00:00:00Z".into(),
        edits: vec![EditRecord {
            file: other.to_string_lossy().into_owned(),
            diff: "+ const x = 1;".into(),
            timestamp: "2026-05-25T00:00:01Z".into(),
        }],
    };

    let _ = engine.check_session(&state);
    assert!(
        !llm_called.load(Ordering::SeqCst),
        "LLM must NOT be called when scope misses"
    );
}
