use hector_core::telemetry::{append, LogEntry, PerRuleRecord, SCHEMA_VERSION as TELEMETRY_SCHEMA};
use hector_core::verdict::{Engine, Status};
use tempfile::tempdir;

#[test]
fn append_creates_log_and_writes_jsonl() {
    let dir = tempdir().unwrap();
    let log = dir.path().join(".hector/log.jsonl");
    let entry = LogEntry::Check {
        ts: "2026-05-11T18:00:00Z".into(),
        file: "src/foo.rs".into(),
        status: Status::Pass,
        elapsed_ms: 12,
        rules: vec![],
    };
    append(&log, &entry).unwrap();
    let content = std::fs::read_to_string(&log).unwrap();
    assert!(content.contains("\"type\":\"check\""));
    assert!(content.contains("\"src/foo.rs\""));

    let entry2 = LogEntry::Check {
        ts: "2026-05-11T18:00:05Z".into(),
        file: "src/bar.rs".into(),
        status: Status::Block,
        elapsed_ms: 22,
        rules: vec![],
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
                    let entry = LogEntry::Check {
                        ts: "t".into(),
                        file: format!("file-{i}-{j}-{}", "x".repeat(8192)),
                        status: Status::Pass,
                        elapsed_ms: 0,
                        rules: vec![],
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
    let entry = LogEntry::Check {
        ts: "t".into(),
        file: "f".into(),
        status: Status::Pass,
        elapsed_ms: 0,
        rules: vec![],
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
    let entry = LogEntry::SemanticSkipped {
        ts: "2026-05-12T00:00:00Z".into(),
        file: "src/lib.rs".into(),
        rule: "no-unwrap".into(),
        reason: "whitespace_only".into(),
    };
    append(&log, &entry).unwrap();
    let content = std::fs::read_to_string(&log).unwrap();
    assert!(content.contains("\"reason\":\"whitespace_only\""));
    assert!(content.contains("\"type\":\"semantic_skipped\""));
}

#[test]
fn telemetry_append_with_parentless_path_returns_error() {
    use std::path::Path;
    let result = append(
        Path::new(""),
        &LogEntry::Check {
            ts: "t".into(),
            file: "f".into(),
            status: Status::Pass,
            elapsed_ms: 0,
            rules: vec![],
        },
    );
    assert!(
        result.is_err(),
        "empty path must surface an error, not panic"
    );
}

#[cfg(unix)]
#[test]
fn telemetry_append_errors_when_parent_uncreatable() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempdir().unwrap();
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let log = tmp.path().join("nested/log.jsonl");
    let entry = LogEntry::Check {
        ts: "t".into(),
        file: "f".into(),
        status: Status::Pass,
        elapsed_ms: 0,
        rules: vec![],
    };
    let result = append(&log, &entry);
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err(), "expected error when parent unwriteable");
}

#[test]
fn semantic_verdict_without_file_omits_field() {
    let dir = tempdir().unwrap();
    let log = dir.path().join(".hector/log.jsonl");
    let entry = LogEntry::SemanticVerdict {
        ts: "2026-05-12T00:00:01Z".into(),
        rule: "no-unwrap".into(),
        verdict: "pass".into(),
        file: None,
    };
    append(&log, &entry).unwrap();
    let content = std::fs::read_to_string(&log).unwrap();
    assert!(
        !content.contains("\"file\""),
        "file omitted when None; line:\n{content}"
    );
}

// --- typed telemetry ------------------------------------------------------

#[test]
fn session_init_round_trips() {
    let entry = LogEntry::SessionInit {
        ts: "2026-05-13T12:00:00Z".into(),
        hector_version: "0.2.2".into(),
        schema_version: TELEMETRY_SCHEMA,
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(
        line.contains("\"type\":\"session_init\""),
        "discriminator field present: {line}"
    );
    assert!(line.contains("\"hector_version\":\"0.2.2\""));
    assert!(line.contains("\"schema_version\":1"));
    let back: LogEntry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry);
}

#[test]
fn check_round_trips_with_per_rule_records() {
    let entry = LogEntry::Check {
        ts: "2026-05-13T12:00:01Z".into(),
        file: "src/lib.rs".into(),
        status: Status::Pass,
        elapsed_ms: 12,
        rules: vec![
            PerRuleRecord {
                rule_id: "no-unwrap".into(),
                engine: Engine::Semantic,
                status: Status::Pass,
                elapsed_ms: 8,
                reason: None,
            },
            PerRuleRecord {
                rule_id: "no-todo".into(),
                engine: Engine::Script,
                status: Status::Warn,
                elapsed_ms: 4,
                reason: None,
            },
        ],
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.contains("\"type\":\"check\""), "discriminator: {line}");
    assert!(line.contains("\"rules\":["));
    assert!(line.contains("\"engine\":\"semantic\""));
    assert!(line.contains("\"engine\":\"script\""));
    let back: LogEntry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry);
}

#[test]
fn check_with_zero_rules_round_trips_and_marks_a_skipped_file() {
    // Skip-pattern fold: file was checked, no rule ran.
    let entry = LogEntry::Check {
        ts: "2026-05-13T12:00:02Z".into(),
        file: "Cargo.lock".into(),
        status: Status::Pass,
        elapsed_ms: 0,
        rules: vec![],
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(
        line.contains("\"rules\":[]"),
        "empty rules array preserved: {line}"
    );
    let back: LogEntry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry);
}

#[test]
fn semantic_verdict_round_trips() {
    let entry = LogEntry::SemanticVerdict {
        ts: "2026-05-13T12:00:03Z".into(),
        rule: "no-secrets".into(),
        verdict: "pass".into(),
        file: Some("src/auth.rs".into()),
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.contains("\"type\":\"semantic_verdict\""));
    assert!(line.contains("\"file\":\"src/auth.rs\""));
    let back: LogEntry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry);
}

#[test]
fn semantic_verdict_with_no_file_round_trips() {
    let entry = LogEntry::SemanticVerdict {
        ts: "2026-05-13T12:00:04Z".into(),
        rule: "session-rule".into(),
        verdict: "pass".into(),
        file: None,
    };
    let line = serde_json::to_string(&entry).unwrap();
    let back: LogEntry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry);
}

#[test]
fn semantic_skipped_round_trips() {
    let entry = LogEntry::SemanticSkipped {
        ts: "2026-05-13T12:00:05Z".into(),
        file: "src/lib.rs".into(),
        rule: "no-unwrap".into(),
        reason: "pure_deletion".into(),
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.contains("\"type\":\"semantic_skipped\""));
    assert!(line.contains("\"reason\":\"pure_deletion\""));
    let back: LogEntry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry);
}

// --- insta snapshots, one per variant -------------------------------------

use insta::assert_json_snapshot;

#[test]
fn snapshot_session_init() {
    let entry = LogEntry::SessionInit {
        ts: "2026-05-13T12:00:00Z".into(),
        hector_version: "0.2.2".into(),
        schema_version: 1,
    };
    assert_json_snapshot!(entry);
}

#[test]
fn snapshot_check_with_rules() {
    let entry = LogEntry::Check {
        ts: "2026-05-13T12:00:01Z".into(),
        file: "src/lib.rs".into(),
        status: Status::Warn,
        elapsed_ms: 42,
        rules: vec![
            PerRuleRecord {
                rule_id: "no-unwrap".into(),
                engine: Engine::Semantic,
                status: Status::Pass,
                elapsed_ms: 30,
                reason: None,
            },
            PerRuleRecord {
                rule_id: "no-todo".into(),
                engine: Engine::Script,
                status: Status::Warn,
                elapsed_ms: 4,
                reason: None,
            },
        ],
    };
    assert_json_snapshot!(entry);
}

#[test]
fn snapshot_check_skip_pattern() {
    let entry = LogEntry::Check {
        ts: "2026-05-13T12:00:02Z".into(),
        file: "Cargo.lock".into(),
        status: Status::Pass,
        elapsed_ms: 0,
        rules: vec![],
    };
    assert_json_snapshot!(entry);
}

#[test]
fn snapshot_semantic_verdict() {
    let entry = LogEntry::SemanticVerdict {
        ts: "2026-05-13T12:00:03Z".into(),
        rule: "no-secrets".into(),
        verdict: "pass".into(),
        file: Some("src/auth.rs".into()),
    };
    assert_json_snapshot!(entry);
}

#[test]
fn snapshot_semantic_skipped() {
    let entry = LogEntry::SemanticSkipped {
        ts: "2026-05-13T12:00:04Z".into(),
        file: "src/lib.rs".into(),
        rule: "no-unwrap".into(),
        reason: "pure_deletion".into(),
    };
    assert_json_snapshot!(entry);
}

#[test]
fn snake_case_field_names_match_spec() {
    // Telemetry fields are snake_case per spec. Pin against accidental rename.
    let entry = LogEntry::SessionInit {
        ts: "t".into(),
        hector_version: "x".into(),
        schema_version: 1,
    };
    let value: serde_json::Value = serde_json::to_value(&entry).unwrap();
    let obj = value.as_object().unwrap();
    assert!(obj.contains_key("type"));
    assert!(obj.contains_key("ts"));
    assert!(obj.contains_key("hector_version"));
    assert!(obj.contains_key("schema_version"));
    assert!(
        !obj.contains_key("hectorVersion"),
        "must be snake_case, not camelCase"
    );
    assert!(!obj.contains_key("hector-version"));
}
