use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn session_start_writes_session_init_telemetry() {
    let dir = tempdir().unwrap();
    let cfg_body = "schema_version: 2\nrules:\n  noop:\n    description: x\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n";
    let trusted = hector_core::trust::write_trust_block(cfg_body).unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(&cfg, trusted).unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .args(["session", "start", "--dir", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let log = fs::read_to_string(dir.path().join(".hector/log.jsonl")).expect("log");
    assert!(log.contains("\"type\":\"session_init\""), "log:\n{log}");
    assert!(
        log.contains("\"hector_version\":"),
        "version stamp present:\n{log}"
    );
    assert!(
        log.contains("\"schema_version\":1"),
        "telemetry schema present:\n{log}"
    );
}

#[test]
fn session_record_lazy_emits_session_init_on_first_edit() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "session",
            "record",
            "--dir",
            dir.path().to_str().unwrap(),
            "--file",
            "src/foo.rs",
            "--diff",
            "--- a/src/foo.rs\n+++ b/src/foo.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n",
        ])
        .assert()
        .success();

    let log = fs::read_to_string(dir.path().join(".hector/log.jsonl")).expect("log");
    assert!(
        log.contains("\"type\":\"session_init\""),
        "first record without prior session.json must emit session_init; log:\n{log}"
    );
}
