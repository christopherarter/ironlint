use hector_core::runner::{CheckInput, HectorEngine};
use std::fs;
use tempfile::tempdir;

#[test]
fn check_fires_for_absolute_input_path() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("src")).unwrap();
    let target = root.join("src/foo.rs");
    fs::write(&target, "fn main() {}\n").unwrap();
    let cfg = r#"schema_version: 2
rules:
  test-rule:
    description: must fire
    engine: script
    scope: ["src/**/*.rs"]
    severity: warning
    script: "exit 1"
"#;
    let cfg_path = root.join(".hector.yml");
    let trusted = hector_core::trust::write_trust_block(cfg).unwrap();
    fs::write(&cfg_path, trusted).unwrap();

    let engine = HectorEngine::load(&cfg_path).unwrap();
    let verdict = engine
        .check(CheckInput::File {
            path: target.clone(),
            content: "fn main() {}\n".to_string(),
        })
        .unwrap();
    // The rule must match for an absolute input path: it lands in violations
    // (exit 1) rather than leaving passed_checks and violations both empty.
    let touched = !verdict.passed_checks.is_empty() || !verdict.violations.is_empty();
    assert!(
        touched,
        "rule must have been evaluated for absolute path, got: {verdict:?}"
    );
}

#[test]
fn check_fires_for_relative_input_path() {
    // Companion to check_fires_for_absolute_input_path: the relative-path
    // call path must keep working through the canonicalize/strip_prefix
    // round-trip used for absolute paths.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("src")).unwrap();
    let abs_target = root.join("src/foo.rs");
    fs::write(&abs_target, "fn main() {}\n").unwrap();
    let cfg = r#"schema_version: 2
rules:
  test-rule:
    description: must fire
    engine: script
    scope: ["src/**/*.rs"]
    severity: warning
    script: "exit 1"
"#;
    let cfg_path = root.join(".hector.yml");
    let trusted = hector_core::trust::write_trust_block(cfg).unwrap();
    fs::write(&cfg_path, trusted).unwrap();
    let engine = HectorEngine::load(&cfg_path).unwrap();
    // Caller passes a relative path (the in-tree case).
    let rel_target = std::path::PathBuf::from("src/foo.rs");
    let verdict = engine
        .check(CheckInput::File {
            path: rel_target,
            content: "fn main() {}\n".to_string(),
        })
        .unwrap();
    let touched = !verdict.passed_checks.is_empty() || !verdict.violations.is_empty();
    assert!(
        touched,
        "rule must fire for relative input path, got: {verdict:?}"
    );
}
