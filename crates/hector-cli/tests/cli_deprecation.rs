use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

/// Schema v1 (legacy bully) is not loadable — it is detected before trust
/// verify and rejected with a clear "run `hector migrate`" hint. Migration
/// is mandatory; loading v1 is a hard error, not a deprecation warning.
#[test]
fn v1_config_check_fails_with_migrate_hint() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(
        &cfg,
        "schema_version: 1\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    )
    .unwrap();
    // Even a trusted v1 config is rejected — schema detection runs before
    // trust verify, by design.
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    let file = dir.path().join("a.txt");
    fs::write(&file, "clean\n").unwrap();
    let output = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(1)
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8_lossy(&output);
    assert!(
        stderr.contains("migrate"),
        "expected `migrate` hint in stderr, got: {stderr}"
    );
}
