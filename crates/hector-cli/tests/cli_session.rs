use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use tempfile::tempdir;

#[test]
fn check_session_consumes_state() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(
        &cfg,
        "schema_version: 2\nrules:\n  noop:\n    description: x\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    ).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();

    fs::create_dir_all(dir.path().join(".hector")).unwrap();
    fs::write(
        dir.path().join(".hector/session.json"),
        r#"{"session_id":"s1","started_at":"t","edits":[{"file":"src/a.ts","diff":"+ const x = 1;","timestamp":"t"}]}"#,
    ).unwrap();

    let output = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--session",
            "--config",
            cfg.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let v: Value = serde_json::from_str(&stdout).expect("parse verdict json");
    assert_eq!(v["status"], "pass", "verdict json: {stdout}");

    assert!(!dir.path().join(".hector/session.json").exists());
}
