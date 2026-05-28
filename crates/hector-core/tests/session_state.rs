use hector_core::session_state::{EditRecord, SessionState};
use tempfile::tempdir;

#[test]
fn session_state_round_trip() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".hector/session.json");
    let mut state = SessionState::new("session-abc");
    state.append(EditRecord {
        file: "src/app.ts".into(),
        diff: "--- a\n+++ b\n+new line".into(),
        timestamp: "2026-05-11T18:00:00Z".into(),
    });
    state.save(&state_path).unwrap();
    let loaded = SessionState::load(&state_path).unwrap();
    assert_eq!(loaded.session_id, "session-abc");
    assert_eq!(loaded.edits.len(), 1);
    assert_eq!(loaded.edits[0].file, "src/app.ts");
}

#[test]
fn session_state_atomic_write() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".hector/session.json");
    let mut s1 = SessionState::new("session-1");
    s1.append(EditRecord {
        file: "a".into(),
        diff: "x".into(),
        timestamp: "t".into(),
    });
    s1.save(&state_path).unwrap();
    let mut s2 = SessionState::new("session-2");
    s2.append(EditRecord {
        file: "b".into(),
        diff: "y".into(),
        timestamp: "u".into(),
    });
    s2.save(&state_path).unwrap();
    let loaded = SessionState::load(&state_path).unwrap();
    assert_eq!(loaded.session_id, "session-2");
}

#[test]
fn session_state_save_caps_edits_at_thousand() {
    // Regression: stored edits must be capped at 1000 (most recent), dropping
    // the oldest. An unbounded `append` lets a long-running session grow
    // session.json without limit and turns the read-modify-write in
    // `hector session record` into O(N^2).
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".hector/session.json");
    let mut state = SessionState::new("long-running");
    for i in 0..10_000 {
        state.append(EditRecord {
            file: format!("src/f{i}.ts"),
            diff: format!("DIFF-{i}"),
            timestamp: "t".into(),
        });
    }
    state.save(&state_path).unwrap();
    let loaded = SessionState::load(&state_path).unwrap();
    assert!(
        loaded.edits.len() <= 1000,
        "expected at most 1000 edits on disk, got {}",
        loaded.edits.len()
    );
    assert_eq!(
        loaded.edits.len(),
        1000,
        "with 10k appends and a 1000-cap we should retain exactly 1000",
    );
    // Most recent must be retained — the last edit (DIFF-9999) is in the file.
    let diffs: Vec<&str> = loaded.edits.iter().map(|e| e.diff.as_str()).collect();
    assert_eq!(
        diffs.last().copied(),
        Some("DIFF-9999"),
        "the newest edit must survive the cap"
    );
    assert!(
        !diffs.contains(&"DIFF-0"),
        "oldest edits should be dropped first"
    );
}

#[test]
fn session_state_load_missing_returns_empty() {
    // Regression: `load` on a non-existent path must return empty state, not
    // an IO error, so `hector check --session` on a fresh checkout just works
    // and adapters don't have to special-case a missing file.
    let dir = tempdir().unwrap();
    let nonexistent = dir.path().join("does/not/exist/session.json");
    let loaded = SessionState::load(&nonexistent).expect("missing file is empty, not error");
    assert!(loaded.edits.is_empty(), "expected no edits on fresh load");
    assert_eq!(loaded.session_id, "");
}

#[test]
fn session_state_clear() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".hector/session.json");
    let mut s = SessionState::new("session-x");
    s.append(EditRecord {
        file: "a".into(),
        diff: "x".into(),
        timestamp: "t".into(),
    });
    s.save(&state_path).unwrap();
    SessionState::clear(&state_path).unwrap();
    // `load` on a missing path yields empty state, not an error.
    assert!(!state_path.exists());
    let loaded = SessionState::load(&state_path).unwrap();
    assert!(loaded.edits.is_empty());
}

#[test]
fn session_state_clear_is_a_noop_when_file_is_absent() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join(".hector/never_existed.json");
    assert!(!state_path.exists());
    SessionState::clear(&state_path).expect("clear must be idempotent on missing");
}

#[test]
fn session_state_save_with_parentless_path_returns_error() {
    use std::path::Path;
    let mut s = SessionState::new("x");
    s.append(EditRecord {
        file: "a".into(),
        diff: "b".into(),
        timestamp: "t".into(),
    });
    let result = s.save(Path::new(""));
    assert!(
        result.is_err(),
        "empty path must surface an error, not panic"
    );
}

#[test]
fn session_state_load_surfaces_parse_error_for_invalid_json() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join("session.json");
    std::fs::write(&state_path, "not json at all").unwrap();
    let err = SessionState::load(&state_path).expect_err("invalid json must error");
    assert!(format!("{err:#}").contains("parsing"));
}
