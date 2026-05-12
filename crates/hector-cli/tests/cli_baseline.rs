use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn baseline_records_and_then_filters() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "DEBUG marker\n").unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(
        &cfg,
        "schema_version: 2\nrules:\n  no-debug:\n    description: x\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"grep -nE 'DEBUG' {file} && exit 1 || exit 0\"\n",
    ).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "baseline",
            "--config",
            cfg.to_str().unwrap(),
            "--scan",
            "*.txt",
        ])
        .assert()
        .success();
    assert!(dir.path().join(".hector/baseline.json").exists());

    Command::cargo_bin("hector")
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
        .code(0);
}
