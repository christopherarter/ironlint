//! Prove that multi-rule dispatch uses the parallel rayon path.
//!
//! The original version of this test measured LLM call overlap via timestamps
//! injected into a mock client. With script rules we use `sleep` to create
//! staggered completion times and rely on the rayon dispatch being wired
//! correctly — correctness (right violations, right rule IDs) is the gate,
//! and a timing assertion (with a generous budget) verifies the parallel path
//! is actually faster than serial.
//!
//! `HECTOR_MAX_WORKERS` env-mutation tests live exclusively in
//! `runner_parallel_order.rs` (a separate test binary / process), so they
//! cannot race with the timing test in this file.

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

/// Five script rules, each sleeping 0.2s then exiting 1 (violation).
/// Serial dispatch would take ≥(5 × 200ms) = 1000ms; parallel should
/// complete well under 500ms on any machine that can compile Rust.
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
    script: "sleep 0.2; exit 1"
  rule-b:
    description: "rule b"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "sleep 0.2; exit 1"
  rule-c:
    description: "rule c"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "sleep 0.2; exit 1"
  rule-d:
    description: "rule d"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "sleep 0.2; exit 1"
  rule-e:
    description: "rule e"
    engine: script
    scope: ["**/*.rs"]
    severity: warning
    script: "sleep 0.2; exit 1"
"#,
    )
}

#[test]
fn five_script_rules_dispatch_in_parallel() {
    // This test assumes HECTOR_MAX_WORKERS is unset in the environment so the
    // engine uses its default thread pool (≥2 workers on any realistic machine).
    // A polluted environment will produce a misleading failure; assert loudly
    // rather than letting a flake hide the root cause.
    assert!(
        std::env::var("HECTOR_MAX_WORKERS").is_err(),
        "HECTOR_MAX_WORKERS must be unset for the parallel timing test to be meaningful"
    );

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

    // Parallel execution: 5 rules each sleeping 200ms.
    // Serial would take ≥1000ms. Allow 500ms as a generous parallel budget —
    // any machine that can compile Rust should dispatch 5 equal-length sleeps
    // in well under that.
    //
    // NOTE: if this assertion flakes on a heavily loaded CI runner, increase
    // the budget rather than removing the assertion — the *behaviour* (parallel
    // dispatch) is correct, only the timing bound may need widening.
    assert!(
        elapsed.as_millis() < 500,
        "parallel dispatch should complete in <500ms; took {}ms (serial would be ≥1000ms)",
        elapsed.as_millis()
    );
}
