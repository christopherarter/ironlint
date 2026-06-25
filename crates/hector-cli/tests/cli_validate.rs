use assert_cmd::Command;
use tempfile::tempdir;

#[test]
fn validate_accepts_valid_gates_config() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(
        &cfg,
        "gates:\n  g:\n    files: [\"**/*.rs\"]\n    run: \"true\"\n",
    )
    .unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(s.contains("ok"), "validate must print ok on success: {s}");
    assert!(
        s.contains("1 gate(s)") || s.contains("1 gate"),
        "validate must print gate count: {s}"
    );
}

#[test]
fn validate_accepts_multi_gate_config() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(
        &cfg,
        "gates:\n  a:\n    files: [\"*.rs\"]\n    run: \"true\"\n  b:\n    files: [\"*.ts\"]\n    run: \"true\"\n",
    )
    .unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(
        s.contains("2 gate(s)") || s.contains("2 gate"),
        "validate must print gate count: {s}"
    );
}

#[test]
fn validate_rejects_legacy_rules_config() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(&cfg, "schema_version: 2\nrules: {}\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn validate_rejects_bad_yaml() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(&cfg, "not: valid: yaml: :\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .assert()
        .failure()
        .code(1);
}
