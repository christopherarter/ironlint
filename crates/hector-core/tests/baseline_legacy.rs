//! Pin the v1 on-disk shape: a baseline written by an older hector (no
//! line-content checksum) must still load and suppress matching violations
//! during the grace period.

use hector_core::baseline::Baseline;
use hector_core::verdict::{Engine, Severity, Violation};
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn make_violation(rule_id: &str, file: &str, line: Option<u32>) -> Violation {
    Violation {
        rule_id: rule_id.to_string(),
        severity: Severity::Warning,
        engine: Engine::Script,
        file: file.to_string(),
        line,
        column: None,
        message: "x".to_string(),
        suggestion: None,
        context: None,
    }
}

#[test]
fn v1_fixture_loads_and_matches_by_tuple_only() {
    let b = Baseline::load(&fixture_path("baseline_v1.json")).expect("v1 fixture must load");
    let v = make_violation("todo-marker", "src/lib.rs", Some(2));
    // Legacy entries have no checksum → "always match" during the grace
    // period. Both same-content and different-content lookups suppress.
    assert!(b.contains_with_content(&v, Some("anything\n")));
    assert!(b.contains_with_content(&v, Some("TODO: changed line\n")));
}
