//! Lock down the determinism + env-override invariants.
//!
//! Determinism is intrinsic to `rayon::par_iter().collect::<Vec<_>>()` (the
//! output order matches input order regardless of completion order), but
//! "rayon" reads as "non-deterministic" at a glance — the order test pins
//! the contract.

use anyhow::Result;
use hector_core::config::Rule;
use hector_core::llm::{LlmClient, RuleVerdict};
use hector_core::runner::{CheckInput, HectorEngine};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::tempdir;

/// LlmClient that returns a Violation for whatever rule it was called with,
/// after sleeping for a per-rule-id-keyed amount. The mock's per-rule sleeps
/// are chosen so completion order (`rule-c, rule-a, rule-b`) differs from
/// input order (BTreeMap key order: `rule-a, rule-b, rule-c`). If the runner
/// ever emitted in completion order, the assertion would fail.
struct OrderingLlm {
    sleeps: Arc<Mutex<Vec<(String, u64)>>>,
    calls: Arc<AtomicUsize>,
}

impl LlmClient for OrderingLlm {
    fn evaluate(
        &self,
        rules: &[(&str, &Rule)],
        _primary: &str,
        _context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // The runner dispatches one rule per call.
        let id = rules[0].0;
        let sleep_ms = self
            .sleeps
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| id.contains(k.as_str()))
            .map(|(_, ms)| *ms)
            .unwrap_or(0);
        std::thread::sleep(Duration::from_millis(sleep_ms));
        Ok(vec![RuleVerdict {
            rule_id: id.to_string(),
            status: hector_core::llm::RuleStatus::Violation {
                message: format!("violation from {id}"),
                line: None,
            },
        }])
    }
}

fn write_three_rule_config(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    let body = r#"schema_version: 2
rules:
  rule-a:
    description: "must X"
    engine: semantic
    scope: ["**/*.rs"]
    severity: warning
    context: diff
  rule-b:
    description: "must Y"
    engine: semantic
    scope: ["**/*.rs"]
    severity: warning
    context: diff
  rule-c:
    description: "must Z"
    engine: semantic
    scope: ["**/*.rs"]
    severity: warning
    context: diff
"#;
    std::fs::write(&path, body).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let with_trust = hector_core::trust::write_trust_block(&raw).unwrap();
    std::fs::write(&path, with_trust).unwrap();
    path
}

fn ordering_engine(cfg_dir: &std::path::Path) -> (HectorEngine, std::path::PathBuf) {
    let cfg = write_three_rule_config(cfg_dir);
    let file = cfg_dir.join("foo.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();

    let sleeps = Arc::new(Mutex::new(vec![
        ("rule-a".to_string(), 30),
        ("rule-b".to_string(), 60),
        ("rule-c".to_string(), 5),
    ]));
    let llm = OrderingLlm {
        sleeps,
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let engine = HectorEngine::builder()
        .with_llm(Box::new(llm))
        .load(&cfg)
        .unwrap();
    (engine, file)
}

const REAL_DIFF: &str = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,2 @@
 fn main() {}
+fn hello() {}
";

#[test]
fn violations_are_ordered_by_rule_id_not_completion_time() {
    let dir = tempdir().unwrap();
    let (engine, file) = ordering_engine(dir.path());

    let verdict = engine
        .check(CheckInput::Diff {
            file,
            unified_diff: REAL_DIFF.to_string(),
        })
        .unwrap();

    let ids: Vec<String> = verdict
        .violations
        .iter()
        .map(|v| v.rule_id.clone())
        .collect();
    // BTreeMap order: rule-a, rule-b, rule-c. Completion order from the mock
    // is rule-c (5ms), rule-a (30ms), rule-b (60ms). Output must reflect
    // input order.
    assert_eq!(
        ids,
        vec![
            "rule-a".to_string(),
            "rule-b".to_string(),
            "rule-c".to_string()
        ],
        "violations must appear in input/rule-id order regardless of completion order"
    );
}

/// `std::env::set_var` is process-global; we don't want HECTOR_MAX_WORKERS
/// to leak into peer tests. Save/restore around the body. The tests in this
/// file are the only ones touching HECTOR_MAX_WORKERS, and cargo test
/// runs each integration-test binary as a separate process (so other binaries
/// can't observe it). The serializing `Mutex` protects against the
/// in-binary parallel test runner.
fn env_mutex() -> &'static std::sync::Mutex<()> {
    static M: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(()))
}

fn with_env<F: FnOnce()>(key: &str, value: Option<&str>, f: F) {
    let _guard = env_mutex().lock().unwrap_or_else(|p| p.into_inner());
    let prev = std::env::var(key).ok();
    match value {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
    f();
    match prev {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
}

#[test]
fn hector_max_workers_env_value_one_still_works() {
    // Sanity that the env override path runs at the smallest valid setting.
    let dir = tempdir().unwrap();
    let (engine, file) = ordering_engine(dir.path());

    with_env("HECTOR_MAX_WORKERS", Some("1"), || {
        let verdict = engine
            .check(CheckInput::Diff {
                file,
                unified_diff: REAL_DIFF.to_string(),
            })
            .unwrap();
        let ids: Vec<String> = verdict
            .violations
            .iter()
            .map(|v| v.rule_id.clone())
            .collect();
        assert_eq!(
            ids,
            vec![
                "rule-a".to_string(),
                "rule-b".to_string(),
                "rule-c".to_string()
            ]
        );
    });
}

#[test]
fn hector_max_workers_zero_value_clamps_to_one_not_deadlock() {
    let dir = tempdir().unwrap();
    let (engine, file) = ordering_engine(dir.path());

    with_env("HECTOR_MAX_WORKERS", Some("0"), || {
        let verdict = engine
            .check(CheckInput::Diff {
                file,
                unified_diff: REAL_DIFF.to_string(),
            })
            .unwrap();
        // Three violations means the run completed (clamping worked).
        assert_eq!(verdict.violations.len(), 3);
    });
}

#[test]
fn hector_max_workers_unparseable_falls_back() {
    let dir = tempdir().unwrap();
    let (engine, file) = ordering_engine(dir.path());

    with_env("HECTOR_MAX_WORKERS", Some("not-a-number"), || {
        let verdict = engine
            .check(CheckInput::Diff {
                file,
                unified_diff: REAL_DIFF.to_string(),
            })
            .unwrap();
        // Unparseable env value falls back to default; the run still completes.
        assert_eq!(verdict.violations.len(), 3);
    });
}
