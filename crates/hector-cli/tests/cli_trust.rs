use assert_cmd::Command;
use tempfile::tempdir;

#[test]
fn trust_writes_fingerprint() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(
        &cfg,
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    ).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();

    let written = std::fs::read_to_string(&cfg).unwrap();
    assert!(written.contains("trust:"), "trust block written");
    assert!(written.contains("sha256:"), "fingerprint written");
}

#[test]
fn trust_then_verify_round_trip() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(
        &cfg,
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    ).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    let written = std::fs::read_to_string(&cfg).unwrap();
    hector_core::trust::verify(&written).expect("verify after trust");
}

#[test]
fn trust_errors_when_config_missing() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("nope.yml");
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", missing.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicates::str::contains("reading"));
}

#[test]
fn trust_errors_on_unparseable_config() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(&cfg, ": : :\n  not yaml\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .failure();
}

#[cfg(unix)]
#[test]
fn trust_errors_when_writing_a_readonly_config_fails() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(
        &cfg,
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    ).unwrap();
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o444)).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicates::str::contains("writing"));
}
