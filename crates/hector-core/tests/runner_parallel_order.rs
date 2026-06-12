//! Lock down the determinism + env-override invariants for parallel dispatch.
//!
//! Determinism is intrinsic to `rayon::par_iter().collect::<Vec<_>>()` (the
//! output order matches input order regardless of completion order), but
//! "rayon" reads as "non-deterministic" at a glance — the order test pins
//! the contract.
//!
//! The original version of this test used a mock LlmClient with per-rule
//! sleep durations. This version replaces semantic rules with script rules
//! using `sleep` shell commands so no LlmClient is required. The contract
//! under test — BTreeMap key order, not completion order — is identical.

use hector_core::runner::{CheckInput, HectorEngine};
use std::fs;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    fs::write(&path, body).unwrap();
    let trusted =
        hector_core::trust::write_trust_block(&fs::read_to_string(&path).unwrap()).unwrap();
    fs::write(&path, trusted).unwrap();
    path
}

/// Three rules with sleep durations chosen so completion order (rule-c first
/// at 5ms, then rule-a at 30ms, then rule-b at 60ms) differs from BTreeMap
/// key order (rule-a, rule-b, rule-c). All rules exit 1 (violation).
fn write_three_rule_config(dir: &std::path::Path) -> std::path::PathBuf {
    write_trusted(
        dir,
        r#"schema_version: 2
rules:
  rule-a:
    description: "must X"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "sleep 0.03; exit 1"
  rule-b:
    description: "must Y"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "sleep 0.06; exit 1"
  rule-c:
    description: "must Z"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "sleep 0.005; exit 1"
"#,
    )
}

fn ordering_engine(cfg_dir: &std::path::Path) -> (HectorEngine, std::path::PathBuf) {
    let cfg = write_three_rule_config(cfg_dir);
    let file = cfg_dir.join("foo.rs");
    fs::write(&file, "fn main() {}\n").unwrap();
    let engine = HectorEngine::load(&cfg).expect("load");
    (engine, file)
}

#[test]
fn violations_are_ordered_by_rule_id_not_completion_time() {
    let dir = tempdir().unwrap();
    let (engine, file) = ordering_engine(dir.path());

    let content = fs::read_to_string(&file).unwrap();
    let verdict = engine
        .check(CheckInput::File {
            path: file,
            content,
        })
        .expect("check");

    let ids: Vec<String> = verdict
        .violations
        .iter()
        .map(|v| v.rule_id.clone())
        .collect();
    // BTreeMap order: rule-a, rule-b, rule-c.
    // Completion order from sleeps: rule-c (5ms), rule-a (30ms), rule-b (60ms).
    // Output must reflect input/rule-id order.
    assert_eq!(
        ids,
        vec![
            "rule-a".to_string(),
            "rule-b".to_string(),
            "rule-c".to_string()
        ],
        "violations must appear in BTreeMap rule-id order regardless of completion order"
    );
}

/// `std::env::set_var` is process-global; we don't want HECTOR_MAX_WORKERS
/// to leak into peer tests. Save/restore around the body. The tests in this
/// file are the only ones touching HECTOR_MAX_WORKERS, and cargo test
/// runs each integration-test binary as a separate process (so other binaries
/// can't observe it). The serialising `Mutex` protects against the
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
    // Ordering is preserved even when forced to 1 thread (serial execution).
    let dir = tempdir().unwrap();
    let (engine, file) = ordering_engine(dir.path());
    let content = fs::read_to_string(&file).unwrap();

    with_env("HECTOR_MAX_WORKERS", Some("1"), || {
        let verdict = engine
            .check(CheckInput::File {
                path: file.clone(),
                content: content.clone(),
            })
            .expect("check");
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
    let content = fs::read_to_string(&file).unwrap();

    with_env("HECTOR_MAX_WORKERS", Some("0"), || {
        let verdict = engine
            .check(CheckInput::File {
                path: file.clone(),
                content: content.clone(),
            })
            .expect("check");
        // Three violations means the run completed (clamping worked).
        assert_eq!(verdict.violations.len(), 3);
    });
}

#[test]
fn hector_max_workers_unparseable_falls_back() {
    let dir = tempdir().unwrap();
    let (engine, file) = ordering_engine(dir.path());
    let content = fs::read_to_string(&file).unwrap();

    with_env("HECTOR_MAX_WORKERS", Some("not-a-number"), || {
        let verdict = engine
            .check(CheckInput::File {
                path: file.clone(),
                content: content.clone(),
            })
            .expect("check");
        // Unparseable env value falls back to default; the run still completes.
        assert_eq!(verdict.violations.len(), 3);
    });
}
