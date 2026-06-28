use hector_core::telemetry::{append, read_all, LogEntry, PerCheckRecord};
use hector_core::verdict::Status;
use std::io::Write;
use tempfile::tempdir;

#[test]
fn append_then_read_all_round_trips() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.jsonl");
    let entry = LogEntry::Check {
        ts: "2026-06-15T00:00:00Z".into(),
        file: "src/lib.rs".into(),
        event: "write".into(),
        status: Status::Block,
        elapsed_ms: 5,
        checks: vec![PerCheckRecord {
            check: "no-todo".into(),
            step: None,
            status: Status::Block,
            elapsed_ms: 5,
            reason: None,
        }],
    };
    append(&log, &entry).unwrap();
    let back = read_all(&log).unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0], entry);
}

#[test]
fn missing_file_returns_empty() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("nonexistent.jsonl");
    let result = read_all(&log).unwrap();
    assert!(result.is_empty());
}

#[test]
fn malformed_line_is_dropped_and_good_lines_survive() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.jsonl");
    let entry = LogEntry::Check {
        ts: "2026-06-15T00:00:00Z".into(),
        file: "a.rs".into(),
        event: "write".into(),
        status: Status::Pass,
        elapsed_ms: 1,
        checks: vec![],
    };
    append(&log, &entry).unwrap();
    // Inject a corrupt line between two good ones.
    let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    writeln!(f, "{{not valid json}}").unwrap();
    drop(f);
    append(&log, &entry).unwrap();

    let back = read_all(&log).unwrap();
    assert_eq!(
        back.len(),
        2,
        "two good entries survive; corrupt line dropped"
    );
}
