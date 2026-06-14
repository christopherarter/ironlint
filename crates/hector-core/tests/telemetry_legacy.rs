use hector_core::telemetry::{read_all, LogEntry};
use hector_core::verdict::Status;
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn legacy_log_jsonl_loads_and_lifts_to_check() {
    let entries = read_all(&fixture_path("log_legacy.jsonl")).expect("legacy fixture must load");
    assert_eq!(
        entries.len(),
        5,
        "all 5 legacy lines must lift, none dropped"
    );

    // Every legacy `kind` now collapses to `Check` — the semantic/session
    // record types were removed with LLM evaluation. `LogEntry` has a single
    // variant, so these destructures are irrefutable.

    // Line 1: kind=check
    let LogEntry::Check {
        file,
        status,
        rules,
        ..
    } = &entries[0];
    assert_eq!(file, "src/foo.rs");
    assert_eq!(*status, Status::Pass);
    assert!(rules.is_empty(), "legacy check has no per-rule data");

    // Line 3: kind=semantic_skipped → Check
    let LogEntry::Check { file, rules, .. } = &entries[2];
    assert_eq!(file, "src/lib.rs");
    assert!(rules.is_empty());

    // Line 4: kind=skipped → Check
    let LogEntry::Check { file, rules, .. } = &entries[3];
    assert_eq!(file, "Cargo.lock");
    assert!(rules.is_empty());

    // Line 5: kind=check_session → Check{file:""}
    let LogEntry::Check {
        file,
        status,
        rules,
        ..
    } = &entries[4];
    assert_eq!(file, "");
    assert_eq!(*status, Status::Block);
    assert!(rules.is_empty());
}

#[test]
fn malformed_legacy_line_is_dropped_with_warning() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log.jsonl");
    let body = "\
{\"timestamp\":\"t\",\"kind\":\"check\",\"file\":\"a\",\"rule_id\":null,\"status\":\"pass\",\"elapsed_ms\":1}
{not valid json
{\"timestamp\":\"t\",\"kind\":\"check\",\"file\":\"b\",\"rule_id\":null,\"status\":\"pass\",\"elapsed_ms\":2}
";
    std::fs::write(&log, body).unwrap();
    let entries = read_all(&log).expect("read_all must succeed even with a bad line");
    assert_eq!(
        entries.len(),
        2,
        "the malformed line is dropped, the others survive"
    );
}

#[test]
fn read_all_returns_empty_for_missing_log() {
    let dir = tempfile::tempdir().unwrap();
    let entries =
        read_all(&dir.path().join("nope.jsonl")).expect("missing file is empty, not error");
    assert!(entries.is_empty());
}

#[test]
fn legacy_warning_fires_only_once() {
    // Write the same legacy line to two distinct files. Read both.
    // Both reads must succeed; the warning is "best-effort" anyway, so
    // we only assert no panic and the expected entry counts.
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.jsonl");
    let b = dir.path().join("b.jsonl");
    let line = "{\"timestamp\":\"t\",\"kind\":\"check\",\"file\":\"x\",\"rule_id\":null,\"status\":\"pass\",\"elapsed_ms\":1}\n";
    std::fs::write(&a, line).unwrap();
    std::fs::write(&b, line).unwrap();
    assert_eq!(read_all(&a).unwrap().len(), 1);
    assert_eq!(read_all(&b).unwrap().len(), 1);
}

#[test]
fn legacy_status_warn_and_block_lift_correctly() {
    // Exercises parse_status's warn and block arms via the legacy reader.
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("statuses.jsonl");
    let body = "{\"timestamp\":\"t\",\"kind\":\"check\",\"file\":\"a\",\"rule_id\":null,\"status\":\"warn\",\"elapsed_ms\":1}\n{\"timestamp\":\"t\",\"kind\":\"check\",\"file\":\"b\",\"rule_id\":null,\"status\":\"block\",\"elapsed_ms\":2}\n";
    std::fs::write(&log, body).unwrap();
    let entries = read_all(&log).unwrap();
    assert_eq!(entries.len(), 2);
    let LogEntry::Check { status: s0, .. } = &entries[0];
    let LogEntry::Check { status: s1, .. } = &entries[1];
    assert_eq!(*s0, Status::Warn);
    assert_eq!(*s1, Status::Block);
}

#[test]
fn legacy_semantic_verdict_collapses_to_check() {
    // The `semantic_verdict` kind has no typed equivalent after LLM evaluation
    // was removed — legacy lines collapse to `Check` via the catch-all lift.
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sv.jsonl");
    let body = "{\"timestamp\":\"t\",\"kind\":\"semantic_verdict\",\"file\":\"src/x.rs\",\"rule_id\":\"r\",\"status\":\"pass\",\"elapsed_ms\":0}\n{\"timestamp\":\"t\",\"kind\":\"semantic_verdict\",\"file\":\"\",\"rule_id\":\"s\",\"status\":\"violation\",\"elapsed_ms\":0}\n";
    std::fs::write(&log, body).unwrap();
    let entries = read_all(&log).unwrap();
    assert_eq!(entries.len(), 2);
    let LogEntry::Check { file, .. } = &entries[0];
    assert_eq!(file, "src/x.rs");
    let LogEntry::Check { file, .. } = &entries[1];
    assert_eq!(file, "");
}
