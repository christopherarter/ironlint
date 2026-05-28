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
