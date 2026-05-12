use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn full_pipeline_script_only() {
    let dir = tempdir().unwrap();
    let project = dir.path();

    let cfg = project.join(".hector.yml");
    fs::write(
        &cfg,
        "schema_version: 2\nrules:\n  no-debug:\n    description: \"no DEBUG markers in source\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"grep -nE 'DEBUG' {file} && exit 1 || exit 0\"\n",
    ).unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();

    let clean = project.join("clean.txt");
    fs::write(&clean, "this file is fine\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            clean.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(0);

    let dirty = project.join("dirty.txt");
    fs::write(&dirty, "TODO: remove DEBUG before commit\n").unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            dirty.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(parsed["status"], "block");
    assert_eq!(parsed["violations"][0]["rule_id"], "no-debug");
    assert_eq!(parsed["violations"][0]["engine"], "script");
    assert_eq!(parsed["violations"][0]["severity"], "error");
}
