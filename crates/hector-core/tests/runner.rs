use hector_core::runner::{CheckInput, HectorEngine};
use hector_core::verdict::Status;
use tempfile::tempdir;

fn write_trusted_config(dir: &std::path::Path, config_body: &str) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    std::fs::write(&path, config_body).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let with_trust = hector_core::trust::write_trust_block(&raw).unwrap();
    std::fs::write(&path, with_trust).unwrap();
    path
}

#[test]
fn passing_rule_yields_pass_verdict() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("foo.txt");
    std::fs::write(&file, "clean\n").unwrap();
    let cfg = "schema_version: 2\nrules:\n  noop:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n";
    let cfg_path = write_trusted_config(dir.path(), cfg);
    let engine = HectorEngine::load(&cfg_path).expect("load");
    let verdict = engine.check(CheckInput::File { path: file.clone(), content: "clean\n".into() });
    assert_eq!(verdict.status, Status::Pass);
}

#[test]
fn failing_rule_yields_block_verdict() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("bad.txt");
    std::fs::write(&file, "forbidden\n").unwrap();
    let cfg = "schema_version: 2\nrules:\n  noforbidden:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"grep -q forbidden {file} && exit 1 || exit 0\"\n";
    let cfg_path = write_trusted_config(dir.path(), cfg);
    let engine = HectorEngine::load(&cfg_path).expect("load");
    let verdict = engine.check(CheckInput::File { path: file.clone(), content: "forbidden\n".into() });
    assert_eq!(verdict.status, Status::Block);
    assert_eq!(verdict.violations.len(), 1);
    assert_eq!(verdict.violations[0].rule_id, "noforbidden");
}

#[test]
fn rule_outside_scope_does_not_fire() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "forbidden\n").unwrap();
    let cfg = "schema_version: 2\nrules:\n  noforbidden:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"grep -q forbidden {file} && exit 1 || exit 0\"\n";
    let cfg_path = write_trusted_config(dir.path(), cfg);
    let engine = HectorEngine::load(&cfg_path).expect("load");
    let verdict = engine.check(CheckInput::File { path: file.clone(), content: "forbidden\n".into() });
    assert_eq!(verdict.status, Status::Pass);
}

#[test]
fn untrusted_config_fails_to_load() {
    let dir = tempdir().unwrap();
    let cfg = "schema_version: 2\nrules: {}\n";
    let cfg_path = dir.path().join(".hector.yml");
    std::fs::write(&cfg_path, cfg).unwrap();
    let result = HectorEngine::load(&cfg_path);
    assert!(result.is_err(), "untrusted config must fail");
}
