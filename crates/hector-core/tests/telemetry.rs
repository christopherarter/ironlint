use hector_core::telemetry::{append, LogEntry};
use tempfile::tempdir;

#[test]
fn append_creates_log_and_writes_jsonl() {
    let dir = tempdir().unwrap();
    let log = dir.path().join(".hector/log.jsonl");
    let entry = LogEntry {
        timestamp: "2026-05-11T18:00:00Z".into(),
        kind: "check".into(),
        file: "src/foo.rs".into(),
        rule_id: None,
        status: "pass".into(),
        elapsed_ms: 12,
        reason: None,
    };
    append(&log, &entry).unwrap();
    let content = std::fs::read_to_string(&log).unwrap();
    assert!(content.contains("\"kind\":\"check\""));
    assert!(content.contains("\"src/foo.rs\""));

    let entry2 = LogEntry {
        timestamp: "2026-05-11T18:00:05Z".into(),
        kind: "check".into(),
        file: "src/bar.rs".into(),
        rule_id: None,
        status: "block".into(),
        elapsed_ms: 22,
        reason: None,
    };
    append(&log, &entry2).unwrap();
    let content = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<_> = content.lines().collect();
    assert_eq!(lines.len(), 2);
}

#[test]
fn telemetry_append_is_atomic_under_concurrent_writers() {
    use std::thread;
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("log.jsonl");
    let handles: Vec<_> = (0..16)
        .map(|i| {
            let p = path.clone();
            thread::spawn(move || {
                for j in 0..100 {
                    let entry = LogEntry {
                        timestamp: "t".into(),
                        kind: "check".into(),
                        file: format!("file-{i}-{j}-{}", "x".repeat(8192)),
                        rule_id: None,
                        status: "pass".into(),
                        elapsed_ms: 0,
                        reason: None,
                    };
                    append(&p, &entry).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let content = std::fs::read_to_string(&path).unwrap();
    let total: usize = content.lines().count();
    assert_eq!(total, 16 * 100, "every line must be written; got {total}");
    for (i, line) in content.lines().enumerate() {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|e| panic!("line {i} not valid JSON: {e}\n{line}"));
    }
}

#[cfg(unix)]
#[test]
fn telemetry_append_creates_file_with_mode_0600() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("log.jsonl");
    let entry = LogEntry {
        timestamp: "t".into(),
        kind: "check".into(),
        file: "f".into(),
        rule_id: None,
        status: "pass".into(),
        elapsed_ms: 0,
        reason: None,
    };
    append(&path, &entry).unwrap();
    let meta = std::fs::metadata(&path).unwrap();
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "telemetry log must be owner-only; got {:o}",
        mode
    );
}

#[test]
fn log_entry_with_reason_serializes_field() {
    let dir = tempdir().unwrap();
    let log = dir.path().join(".hector/log.jsonl");
    let entry = LogEntry {
        timestamp: "2026-05-12T00:00:00Z".into(),
        kind: "semantic_skipped".into(),
        file: "src/lib.rs".into(),
        rule_id: Some("no-unwrap".into()),
        status: "pass".into(),
        elapsed_ms: 0,
        reason: Some("whitespace_only".into()),
    };
    append(&log, &entry).unwrap();
    let content = std::fs::read_to_string(&log).unwrap();
    assert!(content.contains("\"reason\":\"whitespace_only\""));
    assert!(content.contains("\"kind\":\"semantic_skipped\""));
}

#[test]
fn telemetry_append_with_parentless_path_returns_error() {
    use std::path::Path;
    let result = append(
        Path::new(""),
        &LogEntry {
            timestamp: "t".into(),
            kind: "check".into(),
            file: "f".into(),
            rule_id: None,
            status: "pass".into(),
            elapsed_ms: 0,
            reason: None,
        },
    );
    assert!(result.is_err(), "empty path must surface an error, not panic");
}

#[cfg(unix)]
#[test]
fn telemetry_append_errors_when_parent_uncreatable() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempdir().unwrap();
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let log = tmp.path().join("nested/log.jsonl");
    let entry = LogEntry {
        timestamp: "t".into(),
        kind: "check".into(),
        file: "f".into(),
        rule_id: None,
        status: "pass".into(),
        elapsed_ms: 0,
        reason: None,
    };
    let result = append(&log, &entry);
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err(), "expected error when parent unwriteable");
}

#[test]
fn log_entry_without_reason_omits_field() {
    let dir = tempdir().unwrap();
    let log = dir.path().join(".hector/log.jsonl");
    let entry = LogEntry {
        timestamp: "2026-05-12T00:00:01Z".into(),
        kind: "check".into(),
        file: "src/lib.rs".into(),
        rule_id: None,
        status: "pass".into(),
        elapsed_ms: 1,
        reason: None,
    };
    append(&log, &entry).unwrap();
    let content = std::fs::read_to_string(&log).unwrap();
    assert!(!content.contains("\"reason\""));
}
