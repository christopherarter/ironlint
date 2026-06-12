//! End-to-end assertion that a realistic `hector` session writes every
//! typed-telemetry variant to .hector/log.jsonl.

use assert_cmd::Command;
use hector_core::telemetry::{read_all, LogEntry};
use std::fs;
use tempfile::tempdir;

fn write_trusted_config(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let cfg = dir.join(".hector.yml");
    let trusted = hector_core::trust::write_trust_block(body).unwrap();
    fs::write(&cfg, trusted).unwrap();
    cfg
}

#[test]
fn full_session_emits_every_typed_variant() {
    let dir = tempdir().unwrap();

    // 1) `hector session start` → SessionInit.
    Command::cargo_bin("hector")
        .unwrap()
        .args(["session", "start", "--dir", dir.path().to_str().unwrap()])
        .assert()
        .success();

    // 2) `hector check --file foo.txt` → Check with PerRuleRecord.
    let cfg_body = "schema_version: 2\nrules:\n  always-pass:\n    description: x\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n";
    let cfg = write_trusted_config(dir.path(), cfg_body);
    let target = dir.path().join("foo.txt");
    fs::write(&target, "hello\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--file",
            target.to_str().unwrap(),
            "--config",
            cfg.to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .assert()
        .success();

    // 3) Read the log via the typed reader.
    let log = dir.path().join(".hector/log.jsonl");
    let entries = read_all(&log).expect("log readable");

    let has_session_init = entries
        .iter()
        .any(|e| matches!(e, LogEntry::SessionInit { .. }));
    let has_check_with_rule = entries.iter().any(|e| {
        matches!(
            e, LogEntry::Check { rules, .. } if rules.iter().any(|r| r.rule_id == "always-pass")
        )
    });
    assert!(
        has_session_init,
        "missing SessionInit; entries: {entries:#?}"
    );
    assert!(
        has_check_with_rule,
        "missing Check carrying PerRuleRecord{{rule_id:always-pass}}; entries: {entries:#?}"
    );
}
