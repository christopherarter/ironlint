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
fn rejects_config_with_ast_engine() {
    let dir = tempdir().unwrap();
    let cfg = "schema_version: 2\nrules:\n  no-foo:\n    description: \"x\"\n    engine: ast\n    scope: [\"*\"]\n    severity: error\n    pattern: \"foo\"\n";
    let path = write_trusted(dir.path(), cfg);
    let result = HectorEngine::load(&path);
    assert!(result.is_err(), "ast engine not yet implemented; load should fail");
    let err = format!("{:#}", result.err().unwrap());
    assert!(err.to_lowercase().contains("not implemented") || err.contains("unimplemented"), "msg: {err}");
}

#[test]
fn rejects_config_with_semantic_engine() {
    let dir = tempdir().unwrap();
    let cfg = "schema_version: 2\nrules:\n  use-derived:\n    description: \"x\"\n    engine: semantic\n    scope: [\"*\"]\n    severity: error\n";
    let path = write_trusted(dir.path(), cfg);
    let result = HectorEngine::load(&path);
    assert!(result.is_err());
}

#[test]
fn rejects_config_with_session_engine() {
    let dir = tempdir().unwrap();
    let cfg = "schema_version: 2\nrules:\n  audit-tests:\n    description: \"x\"\n    engine: session\n    scope: [\"*\"]\n    severity: error\n";
    let path = write_trusted(dir.path(), cfg);
    let result = HectorEngine::load(&path);
    assert!(result.is_err());
}
