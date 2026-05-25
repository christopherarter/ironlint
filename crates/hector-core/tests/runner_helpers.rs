use hector_core::runner::HectorEngine;
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn resolve_input_path_returns_absolute_unchanged() {
    let tmp = tempdir().unwrap();
    let config = write_trusted_minimal_config(tmp.path());
    let engine = HectorEngine::load(&config).expect("load");
    let abs = PathBuf::from("/some/absolute/path.rs");
    // For now, returns as-is. C4 (Phase 2 Task 2.3) will add the
    // outside-config_dir error gate.
    let resolved = engine.resolve_input_path(&abs);
    assert_eq!(resolved, abs);
}

#[test]
fn resolve_input_path_joins_relative_onto_config_dir() {
    let tmp = tempdir().unwrap();
    let config = write_trusted_minimal_config(tmp.path());
    let engine = HectorEngine::load(&config).expect("load");
    let rel = PathBuf::from("src/lib.rs");
    let resolved = engine.resolve_input_path(&rel);
    assert_eq!(resolved, tmp.path().join("src/lib.rs"));
}

fn write_trusted_minimal_config(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    let yaml = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n";
    std::fs::write(&path, yaml).unwrap();
    let signed = hector_core::trust::write_trust_block(yaml).unwrap();
    std::fs::write(&path, signed).unwrap();
    path
}
