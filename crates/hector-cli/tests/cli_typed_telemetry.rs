//! End-to-end assertion that a realistic `hector check` run writes a
//! typed-telemetry Check variant to .hector/log.jsonl.

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
fn check_emits_typed_check_variant() {
    let dir = tempdir().unwrap();

    // `hector check --file foo.txt` → Check with PerRuleRecord.
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

    // Read the log via the typed reader.
    let log = dir.path().join(".hector/log.jsonl");
    let entries = read_all(&log).expect("log readable");

    let has_check_with_rule = entries.iter().any(|e| {
        matches!(
            e, LogEntry::Check { rules, .. } if rules.iter().any(|r| r.rule_id == "always-pass")
        )
    });
    assert!(
        has_check_with_rule,
        "missing Check carrying PerRuleRecord{{rule_id:always-pass}}; entries: {entries:#?}"
    );
}
