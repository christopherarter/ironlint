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

#[test]
fn semantic_skipped_record_is_emitted_for_pure_deletion_diff() {
    let dir = tempdir().unwrap();

    // Semantic rule with "avoid" phrasing → pure-deletion diffs short-circuit
    // without dispatching to the LLM.
    let cfg_body = r#"schema_version: 2
llm:
  provider: anthropic
  model: claude-haiku
  api_key_env: HECTOR_FAKE_KEY
rules:
  no-todo:
    description: "avoid TODO comments"
    engine: semantic
    scope: ["*.rs"]
    severity: warning
"#;
    let cfg = write_trusted_config(dir.path(), cfg_body);

    let target = dir.path().join("foo.rs");
    fs::write(&target, "fn main() {}\n").unwrap();

    let diff_path = dir.path().join("change.diff");
    fs::write(
        &diff_path,
        "--- a/foo.rs\n+++ b/foo.rs\n@@ -1,2 +1,1 @@\n fn main() {}\n-let x = 1;\n",
    )
    .unwrap();

    // Expected to short-circuit before hitting the LLM (pure deletion + "avoid").
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--diff",
            diff_path.to_str().unwrap(),
            "--config",
            cfg.to_str().unwrap(),
        ])
        .env("HECTOR_FAKE_KEY", "x") // never read; LLM never dispatched
        .current_dir(dir.path())
        .assert()
        .success();

    let log = dir.path().join(".hector/log.jsonl");
    let entries = read_all(&log).expect("log readable");
    let has_skip = entries.iter().any(|e| {
        matches!(
            e, LogEntry::SemanticSkipped { rule, reason, .. }
            if rule == "no-todo" && reason == "pure_deletion"
        )
    });
    assert!(
        has_skip,
        "missing SemanticSkipped{{pure_deletion}}; entries: {entries:#?}"
    );
}
