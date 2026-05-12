use hector_core::baseline::Baseline;
use hector_core::verdict::{Engine, Severity, Violation};
use tempfile::tempdir;

fn make_violation(rule_id: &str, file: &str, line: Option<u32>) -> Violation {
    Violation {
        rule_id: rule_id.to_string(),
        severity: Severity::Error,
        engine: Engine::Script,
        file: file.to_string(),
        line,
        column: None,
        message: "boom".to_string(),
        suggestion: None,
        context: None,
    }
}

#[test]
fn default_baseline_contains_nothing() {
    let b = Baseline::default();
    let v = make_violation("r1", "a.txt", Some(3));
    assert!(!b.contains(&v));
}

#[test]
fn add_then_contains_is_true() {
    let mut b = Baseline::default();
    let v = make_violation("r1", "a.txt", Some(3));
    b.add(&v);
    assert!(b.contains(&v));
}

#[test]
fn fingerprint_is_stable_for_identical_violations() {
    let v1 = make_violation("r1", "a.txt", Some(3));
    let mut v2 = make_violation("r1", "a.txt", Some(3));
    // Differ in fields the fingerprint must ignore.
    v2.message = "different message".to_string();
    v2.severity = Severity::Warning;
    v2.engine = Engine::Ast;
    v2.column = Some(99);
    v2.suggestion = Some("hint".to_string());
    v2.context = Some("ctx".to_string());
    assert_eq!(Baseline::fingerprint(&v1), Baseline::fingerprint(&v2));
}

#[test]
fn load_missing_path_returns_default() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("does_not_exist.json");
    let b = Baseline::load(&path).expect("missing path is OK");
    assert!(b.fingerprints.is_empty());
}

#[test]
fn save_creates_parent_dir() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(".hector").join("baseline.json");
    assert!(!path.parent().unwrap().exists());
    let b = Baseline::default();
    b.save(&path).expect("save should create parent dir");
    assert!(path.exists());
}

#[test]
fn save_then_load_round_trip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(".hector").join("baseline.json");
    let mut b = Baseline::default();
    let v1 = make_violation("rule-a", "a.txt", Some(1));
    let v2 = make_violation("rule-b", "b.txt", Some(2));
    let v3 = make_violation("rule-c", "c.txt", None);
    b.add(&v1);
    b.add(&v2);
    b.add(&v3);
    b.save(&path).unwrap();
    let loaded = Baseline::load(&path).unwrap();
    assert!(loaded.contains(&v1));
    assert!(loaded.contains(&v2));
    assert!(loaded.contains(&v3));
}

// Current fingerprint formula is `{rule_id}::{file}::{line.unwrap_or(0)}`,
// so a Violation with `line: None` collides with one carrying `line: Some(0)`.
// This pins the present behavior — it is intentional bookkeeping, not a bug:
// callers that need to baseline a no-line violation and a line-0 violation
// separately would have to disambiguate at the fingerprint layer.
#[test]
fn line_none_collides_with_line_zero() {
    let v_none = make_violation("r1", "a.txt", None);
    let v_zero = make_violation("r1", "a.txt", Some(0));
    assert_eq!(
        Baseline::fingerprint(&v_none),
        Baseline::fingerprint(&v_zero)
    );
}
