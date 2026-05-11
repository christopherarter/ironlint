use assert_cmd::Command;
use tempfile::tempdir;

#[test]
fn validate_accepts_good_config() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(
        &cfg,
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    ).unwrap();
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
    std::fs::write(&cfg, "schema_version: 99\nrules: {}\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .assert()
        .failure()
        .code(1);
}
