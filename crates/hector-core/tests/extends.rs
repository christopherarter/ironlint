use hector_core::config::parse_file_with_extends;
use std::path::PathBuf;

fn workspace_fixture(rel: &str) -> PathBuf {
    // CARGO_MANIFEST_DIR is `crates/hector-core/`; fixtures live at workspace root.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.join("../..").join(rel)
}

#[test]
fn extends_merges_rules() {
    let path = workspace_fixture("tests/fixtures/with_extends/child.hector.yml");
    let cfg = parse_file_with_extends(&path).expect("parse");
    assert!(cfg.rules.contains_key("base-rule"), "base rule inherited");
    assert!(cfg.rules.contains_key("child-rule"), "child rule present");
}

#[test]
fn cycle_in_extends_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.yml");
    let b = dir.path().join("b.yml");
    std::fs::write(&a, "schema_version: 2\nextends: [./b.yml]\nrules: {}\n").unwrap();
    std::fs::write(&b, "schema_version: 2\nextends: [./a.yml]\nrules: {}\n").unwrap();
    let result = parse_file_with_extends(&a);
    assert!(result.is_err(), "cycle detection should fail");
    let err = format!("{:#}", result.unwrap_err());
    assert!(
        err.to_lowercase().contains("cycle") || err.to_lowercase().contains("loop"),
        "error mentions cycle: {err}"
    );
}
