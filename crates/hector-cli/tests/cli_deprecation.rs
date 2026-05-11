use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

fn write_trusted_v1(dir: &std::path::Path) -> std::path::PathBuf {
    let cfg = dir.join(".hector.yml");
    fs::write(
        &cfg,
        "schema_version: 1\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    ).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    cfg
}

#[test]
fn v1_config_emits_deprecation_warning() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted_v1(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "clean\n").unwrap();
    let output = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config", cfg.to_str().unwrap(),
            "--file", file.to_str().unwrap(),
            "--format", "json",
        ])
        .assert()
        .code(0)
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8_lossy(&output);
    assert!(
        stderr.contains("deprecated") || stderr.contains("legacy") || stderr.contains("hector migrate"),
        "expected deprecation in stderr, got: {stderr}"
    );
}
