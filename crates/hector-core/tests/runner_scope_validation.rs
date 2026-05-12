use hector_core::runner::HectorEngine;
use std::fs;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    fs::write(&path, body).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    let with_trust = hector_core::trust::write_trust_block(&raw).unwrap();
    fs::write(&path, with_trust).unwrap();
    path
}

#[test]
fn invalid_glob_in_scope_fails_load() {
    let dir = tempdir().unwrap();
    let cfg = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"[abc\"]\n    severity: error\n    script: \"true\"\n";
    let path = write_trusted(dir.path(), cfg);
    let result = HectorEngine::load(&path);
    assert!(result.is_err(), "invalid glob should fail load");
    let err = format!("{:#}", result.err().unwrap());
    assert!(
        err.to_lowercase().contains("glob") || err.to_lowercase().contains("scope"),
        "msg: {err}"
    );
}
