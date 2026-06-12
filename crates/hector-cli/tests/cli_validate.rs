use assert_cmd::Command;
use tempfile::tempdir;

#[test]
fn validate_accepts_good_config() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    let raw = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n";
    let trusted = hector_core::trust::write_trust_block(raw).unwrap();
    std::fs::write(&cfg, trusted).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn validate_rejects_bad_config() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    let raw = "schema_version: 99\nrules: {}\n";
    let trusted = hector_core::trust::write_trust_block(raw).unwrap();
    std::fs::write(&cfg, trusted).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .assert()
        .failure()
        .code(1);
}

/// `validate` must reject a config with an untrusted parent — same
/// loadability contract as `check`.
#[test]
fn validate_rejects_untrusted_extends_parent() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.yml");
    std::fs::write(
        &parent,
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    )
    .unwrap();
    let child = dir.path().join(".hector.yml");
    let raw_child = "schema_version: 2\nextends: [\"parent.yml\"]\nrules: {}\n";
    let trusted_child = hector_core::trust::write_trust_block(raw_child).unwrap();
    std::fs::write(&child, trusted_child).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["validate", "--config", child.to_str().unwrap()])
        .assert()
        .failure()
        .code(1);
}

/// `validate` must surface the parse-time error for removed engines (semantic/session).
#[test]
fn validate_rejects_semantic_engine_with_curated_error() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    let raw = "schema_version: 2\nrules:\n  judge-me:\n    description: \"llm rule\"\n    engine: semantic\n    scope: [\"**/*.ts\"]\n    severity: error\n";
    let trusted = hector_core::trust::write_trust_block(raw).unwrap();
    std::fs::write(&cfg, trusted).unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "semantic engine must exit 1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("engine 'semantic' was removed"),
        "stderr must carry curated error; got: {stderr}"
    );
}
