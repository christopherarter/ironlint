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
//!
//! ALL `HECTOR_MAX_WORKERS` env-mutation tests live in this file. Because
//! each integration-test file compiles to its own binary (separate process),
//! mutations here cannot race with the timing test in `runner_parallel.rs`.
//! The in-binary `env_mutex` + `with_env` helpers serialise the mutations
//! against each other within this process.

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

/// Five script rules used to verify env-override paths with a richer rule set.
fn write_five_rule_config(dir: &std::path::Path) -> std::path::PathBuf {
    write_trusted(
        dir,
        r#"schema_version: 2
rules:
  rule-a:
    description: "rule a"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "exit 1"
  rule-b:
    description: "rule b"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "exit 1"
  rule-c:
    description: "rule c"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "exit 1"
  rule-d:
    description: "rule d"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "exit 1"
  rule-e:
    description: "rule e"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "exit 1"
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

/// `std::env::set_var` is process-global; save/restore around the body and
/// serialise in-process env mutations via a Mutex so the parallel test runner
/// can't interleave them.
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

/// Helper: run the five-rule config under a given env value, return violation count.
fn run_five_rules_under_env(env_value: Option<&str>) -> usize {
    let dir = tempdir().unwrap();
    let cfg = write_five_rule_config(dir.path());
    let file = dir.path().join("foo.rs");
    fs::write(&file, "fn main() {}\n").unwrap();
    let engine = HectorEngine::load(&cfg).expect("load");
    let content = fs::read_to_string(&file).unwrap();
    let mut count = 0;
    with_env("HECTOR_MAX_WORKERS", env_value, || {
        let verdict = engine
            .check(CheckInput::File {
                path: file.clone(),
                content: content.clone(),
            })
            .expect("check");
        count = verdict.violations.len();
    });
    count
}

// ---------------------------------------------------------------------------
// Ordering tests
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// HECTOR_MAX_WORKERS env-override tests
// ---------------------------------------------------------------------------

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
fn hector_max_workers_numeric_env_completes_correctly() {
    // HECTOR_MAX_WORKERS=2 limits to 2 threads but must still produce all 5
    // violations (just serially queued through 2 workers).
    let count = run_five_rules_under_env(Some("2"));
    assert_eq!(count, 5, "expected 5 violations with HECTOR_MAX_WORKERS=2");
}

#[test]
fn hector_max_workers_zero_clamps_to_one_not_deadlock() {
    // HECTOR_MAX_WORKERS=0 is clamped to 1; must not panic and must produce
    // all violations.
    let count = run_five_rules_under_env(Some("0"));
    assert_eq!(
        count, 5,
        "expected 5 violations with HECTOR_MAX_WORKERS=0 (clamped to 1)"
    );
}

#[test]
fn hector_max_workers_unparseable_falls_back_to_default() {
    // An unparseable value falls back to the default pool size; check still
    // completes and produces all violations.
    let count = run_five_rules_under_env(Some("not-a-number"));
    assert_eq!(
        count, 5,
        "expected 5 violations with HECTOR_MAX_WORKERS=not-a-number (fallback)"
    );
}

// ---------------------------------------------------------------------------
// execution.max_workers config-field test
// ---------------------------------------------------------------------------

#[test]
fn execution_max_workers_config_field_is_honoured() {
    // `execution: {max_workers: 1}` in the YAML limits to one thread.
    // The check must still complete and return all violations.
    //
    // This test holds the env_mutex because execution_pool() reads
    // HECTOR_MAX_WORKERS first (env takes precedence over config), so a
    // concurrent env-mutating test in the same binary could shadow the config
    // field and invalidate the assertion.
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        r#"schema_version: 2
execution:
  max_workers: 1
rules:
  rule-a:
    description: "rule a"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "exit 1"
  rule-b:
    description: "rule b"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "exit 1"
  rule-c:
    description: "rule c"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "exit 1"
"#,
    );
    let file = dir.path().join("foo.rs");
    fs::write(&file, "fn main() {}\n").unwrap();
    let engine = HectorEngine::load(&cfg).expect("load");

    with_env("HECTOR_MAX_WORKERS", None, || {
        let verdict = engine
            .check(CheckInput::File {
                path: file.clone(),
                content: fs::read_to_string(&file).unwrap(),
            })
            .expect("check");
        assert_eq!(
            verdict.violations.len(),
            3,
            "expected 3 violations with execution.max_workers=1"
        );
    });
}
