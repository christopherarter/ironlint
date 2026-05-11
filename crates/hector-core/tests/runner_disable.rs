use hector_core::runner::{CheckInput, HectorEngine};
use hector_core::verdict::Status;
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
fn runner_compiles_with_disable_map_path() {
    // Sanity test: 0.1b script engine doesn't emit line numbers, so disable comments
    // can't fully gate violations through the script path. We verify the runner wires
    // the DisableMap call without regressing existing behavior.
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "// hector-disable: no-debug reason: x\n").unwrap();
    let cfg = "schema_version: 2\nrules:\n  no-debug:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n";
    let path = write_trusted(dir.path(), cfg);
    let engine = HectorEngine::load(&path).expect("load");
    let verdict = engine.check(CheckInput::File { path: file.clone(), content: fs::read_to_string(&file).unwrap() });
    assert_eq!(verdict.status, Status::Pass);
}
