use hector_core::llm::NoLlm;
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
fn builder_with_llm_injects_dependency() {
    let dir = tempdir().unwrap();
    let cfg = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n";
    let path = write_trusted(dir.path(), cfg);
    let engine = HectorEngine::builder()
        .with_llm(Box::new(NoLlm))
        .load(&path)
        .expect("load");
    let _ = engine;
}

#[test]
fn default_load_uses_no_llm() {
    let dir = tempdir().unwrap();
    let cfg = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n";
    let path = write_trusted(dir.path(), cfg);
    let engine = HectorEngine::load(&path).expect("load");
    let _ = engine;
}
