//! End-to-end coverage that `hector record-verdict` appends a
//! `SemanticVerdict` line to `.hector/log.jsonl`.

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn record_verdict_subcommand_is_recognised() {
    // Phase 1: just confirm the subcommand exists. Real append is verified
    // in Phase 2's test (overrides this minimal check).
    let tmp = tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .arg("record-verdict")
        .arg("--rule")
        .arg("no-debug")
        .arg("--verdict")
        .arg("pass")
        .arg("--dir")
        .arg(tmp.path())
        .assert()
        .code(0);
}

#[test]
fn record_verdict_rejects_invalid_verdict_value() {
    // clap-enforced. Anything other than `pass` or `violation` errors at
    // parse time. We do NOT use code 1 here — clap exits with its own code
    // (2 on most platforms) for parse errors. The body of `run` is never
    // entered.
    let tmp = tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .arg("record-verdict")
        .arg("--rule")
        .arg("no-debug")
        .arg("--verdict")
        .arg("fail") // not in the enum
        .arg("--dir")
        .arg(tmp.path())
        .assert()
        .failure();
}

#[test]
fn record_verdict_appends_one_semantic_verdict_line() {
    let tmp = tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .arg("record-verdict")
        .arg("--rule")
        .arg("no-debug")
        .arg("--verdict")
        .arg("violation")
        .arg("--file")
        .arg("src/foo.rs")
        .arg("--dir")
        .arg(tmp.path())
        .assert()
        .code(0);

    let log_path = tmp.path().join(".hector/log.jsonl");
    assert!(log_path.exists(), ".hector/log.jsonl must be created");
    let content = fs::read_to_string(&log_path).unwrap();
    // The log MAY contain a leading `session_init` record (lazy stamp).
    // We assert there is exactly one `semantic_verdict` line and that its
    // fields match what we passed in.
    let semantic_lines: Vec<&str> = content
        .lines()
        .filter(|l| l.contains("\"type\":\"semantic_verdict\""))
        .collect();
    assert_eq!(
        semantic_lines.len(),
        1,
        "expected exactly one semantic_verdict line, got: {content}"
    );
    let v: serde_json::Value = serde_json::from_str(semantic_lines[0]).unwrap();
    assert_eq!(v["type"], "semantic_verdict");
    assert_eq!(v["rule"], "no-debug");
    assert_eq!(v["verdict"], "violation");
    assert_eq!(v["file"], "src/foo.rs");
    assert!(
        v["ts"].as_str().unwrap().contains('T'),
        "ts must be rfc3339 (contains 'T'), got {:?}",
        v["ts"]
    );
}

#[test]
fn record_verdict_with_no_file_omits_field() {
    // `LogEntry::SemanticVerdict.file` is `Option<String>` serialized with
    // `skip_serializing_if = "Option::is_none"` — when omitted on the
    // command line, the on-disk line has no `file` key at all.
    let tmp = tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .arg("record-verdict")
        .arg("--rule")
        .arg("no-debug")
        .arg("--verdict")
        .arg("pass")
        .arg("--dir")
        .arg(tmp.path())
        .assert()
        .code(0);

    let content = fs::read_to_string(tmp.path().join(".hector/log.jsonl")).unwrap();
    let line = content
        .lines()
        .find(|l| l.contains("\"type\":\"semantic_verdict\""))
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert!(
        v.get("file").is_none(),
        "file key must be absent when --file is omitted; got {line}"
    );
}

#[test]
fn record_verdict_writes_session_init_lazily() {
    // The first record-verdict in a fresh project stamps a session_init
    // record before its semantic_verdict, so `hector coverage` and the
    // legacy-format lifter see a well-formed log.
    let tmp = tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .arg("record-verdict")
        .arg("--rule")
        .arg("r1")
        .arg("--verdict")
        .arg("pass")
        .arg("--dir")
        .arg(tmp.path())
        .assert()
        .code(0);

    let content = fs::read_to_string(tmp.path().join(".hector/log.jsonl")).unwrap();
    let first_line = content.lines().next().expect("at least one line");
    let v: serde_json::Value = serde_json::from_str(first_line).unwrap();
    assert_eq!(
        v["type"], "session_init",
        "first record in a fresh log must be session_init, got: {first_line}"
    );
}

#[cfg(unix)]
#[test]
fn record_verdict_returns_1_on_telemetry_write_failure() {
    // Point --dir at a read-only directory. telemetry::append's
    // create_dir_all + open(append) chain will fail, and run() must
    // return 1 with a stderr message.
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempdir().unwrap();
    let readonly = tmp.path().join("readonly");
    std::fs::create_dir(&readonly).unwrap();
    let mut perms = std::fs::metadata(&readonly).unwrap().permissions();
    perms.set_mode(0o500); // r-x for owner; no write
    std::fs::set_permissions(&readonly, perms).unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .arg("record-verdict")
        .arg("--rule")
        .arg("r1")
        .arg("--verdict")
        .arg("pass")
        .arg("--dir")
        .arg(&readonly)
        .assert()
        .code(1)
        .stderr(predicates::str::contains("ERROR:"));

    // Cleanup: restore write so the tempdir teardown succeeds.
    let mut perms = std::fs::metadata(&readonly).unwrap().permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(&readonly, perms).unwrap();
}
