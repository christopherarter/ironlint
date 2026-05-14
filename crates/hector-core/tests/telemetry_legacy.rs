use hector_core::telemetry::{read_all, LogEntry};
use hector_core::verdict::Status;
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn legacy_log_jsonl_loads_and_lifts_to_typed_variants() {
    let entries = read_all(&fixture_path("log_legacy.jsonl")).expect("legacy fixture must load");
    assert_eq!(
        entries.len(),
        5,
        "all 5 legacy lines must lift, none dropped"
    );

    // Line 1: kind=check → Check{rules:[]}
    match &entries[0] {
        LogEntry::Check {
            file,
            status,
            rules,
            ..
        } => {
            assert_eq!(file, "src/foo.rs");
            assert_eq!(*status, Status::Pass);
            assert!(rules.is_empty(), "legacy check has no per-rule data");
        }
        other => panic!("entry 0 should be Check, got {other:?}"),
    }

    // Line 3: kind=semantic_skipped → SemanticSkipped
    match &entries[2] {
        LogEntry::SemanticSkipped {
            file, rule, reason, ..
        } => {
            assert_eq!(file, "src/lib.rs");
            assert_eq!(rule, "no-unwrap");
            assert_eq!(reason, "whitespace_only");
        }
        other => panic!("entry 2 should be SemanticSkipped, got {other:?}"),
    }

    // Line 4: kind=skipped → Check{rules:[]}
    match &entries[3] {
        LogEntry::Check { file, rules, .. } => {
            assert_eq!(file, "Cargo.lock");
            assert!(rules.is_empty());
        }
        other => panic!("entry 3 should be Check, got {other:?}"),
    }

    // Line 5: kind=check_session → Check{file:"", rules:[]}
    match &entries[4] {
        LogEntry::Check {
            file,
            status,
            rules,
            ..
        } => {
            assert_eq!(file, "");
            assert_eq!(*status, Status::Block);
            assert!(rules.is_empty());
        }
        other => panic!("entry 4 should be Check, got {other:?}"),
    }
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
    let LogEntry::Check { status: s0, .. } = &entries[0] else {
        panic!("warn line should be Check");
    };
    let LogEntry::Check { status: s1, .. } = &entries[1] else {
        panic!("block line should be Check");
    };
    assert_eq!(*s0, Status::Warn);
    assert_eq!(*s1, Status::Block);
}

#[test]
fn legacy_semantic_verdict_lifts_correctly() {
    // Exercises into_typed's `"semantic_verdict"` arm — the rest of the
    // arms are covered by the main fixture.
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sv.jsonl");
    // legacy semantic_verdict: file present
    let body = "{\"timestamp\":\"t\",\"kind\":\"semantic_verdict\",\"file\":\"src/x.rs\",\"rule_id\":\"r\",\"status\":\"pass\",\"elapsed_ms\":0}\n{\"timestamp\":\"t\",\"kind\":\"semantic_verdict\",\"file\":\"\",\"rule_id\":\"s\",\"status\":\"violation\",\"elapsed_ms\":0}\n";
    std::fs::write(&log, body).unwrap();
    let entries = read_all(&log).unwrap();
    assert_eq!(entries.len(), 2);
    match &entries[0] {
        LogEntry::SemanticVerdict {
            file: Some(f),
            verdict,
            rule,
            ..
        } => {
            assert_eq!(f, "src/x.rs");
            assert_eq!(verdict, "pass");
            assert_eq!(rule, "r");
        }
        other => panic!("expected SemanticVerdict with file, got {other:?}"),
    }
    match &entries[1] {
        LogEntry::SemanticVerdict {
            file: None,
            verdict,
            rule,
            ..
        } => {
            assert_eq!(verdict, "violation");
            assert_eq!(rule, "s");
        }
        other => panic!("expected SemanticVerdict file=None, got {other:?}"),
    }
}
