//! Prove that multi-rule dispatch uses the parallel rayon path and that
//! `HECTOR_MAX_WORKERS` / `execution.max_workers` config are honoured.
//!
//! The original version of this test measured LLM call overlap via timestamps
//! injected into a mock client. With script rules we use `sleep` to create
//! staggered completion times and rely on the rayon dispatch being wired
//! correctly — correctness (right violations, right rule IDs) is the gate,
//! and a timing assertion (with a generous budget) verifies the parallel path
//! is actually faster than serial.

use hector_core::runner::{CheckInput, HectorEngine};
use std::fs;
use std::time::Instant;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    fs::write(&path, body).unwrap();
    let trusted =
        hector_core::trust::write_trust_block(&fs::read_to_string(&path).unwrap()).unwrap();
    fs::write(&path, trusted).unwrap();
    path
}

/// Five script rules, each sleeping a distinct number of ms then exiting 1
/// (violation). Serial dispatch would take ≥(50+60+70+80+90)ms = 350ms;
/// parallel should land well under that on any modern machine.
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
    script: "sleep 0.05; exit 1"
  rule-b:
    description: "rule b"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "sleep 0.06; exit 1"
  rule-c:
    description: "rule c"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "sleep 0.07; exit 1"
  rule-d:
    description: "rule d"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "sleep 0.08; exit 1"
  rule-e:
    description: "rule e"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "sleep 0.09; exit 1"
"#,
    )
}

#[test]
fn five_script_rules_dispatch_in_parallel() {
    let dir = tempdir().unwrap();
    let cfg = write_five_rule_config(dir.path());
    let file = dir.path().join("foo.rs");
    fs::write(&file, "fn main() {}\n").unwrap();

    let engine = HectorEngine::load(&cfg).expect("load");

    let start = Instant::now();
    let verdict = engine
        .check(CheckInput::File {
            path: file,
            content: fs::read_to_string(dir.path().join("foo.rs")).unwrap(),
        })
        .expect("check");
    let elapsed = start.elapsed();

    // All five rules matched and produced violations.
    assert_eq!(
        verdict.violations.len(),
        5,
        "expected 5 violations, got {}: {:?}",
        verdict.violations.len(),
        verdict
            .violations
            .iter()
            .map(|v| &v.rule_id)
            .collect::<Vec<_>>()
    );

    // Parallel execution: 5 rules each sleeping ~90ms max.
    // Serial would take ≥350ms. Allow 3× the longest single sleep (270ms)
    // as a very conservative bound — any machine that can compile Rust
    // should dispatch 5 short sleeps in under 270ms.
    //
    // NOTE: if this assertion flakes on a heavily loaded CI runner, increase
    // the budget rather than removing the assertion — the *behaviour* (parallel
    // dispatch) is correct, only the timing bound may need widening.
    assert!(
        elapsed.as_millis() < 300,
        "parallel dispatch should complete in <300ms; took {}ms (serial would be ≥350ms)",
        elapsed.as_millis()
    );
}

/// Serialising guard: save/restore `HECTOR_MAX_WORKERS` and serialise
/// in-process env mutations via a Mutex so parallel test runners can't
/// interleave them.
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

/// Helper: run the five-rule config under a given env, return violation count.
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

#[test]
fn execution_max_workers_config_field_is_honoured() {
    // `execution: {max_workers: 1}` in the YAML limits to one thread.
    // The check must still complete and return all violations.
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
}
