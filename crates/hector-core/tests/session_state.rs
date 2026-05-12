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
    assert!(SessionState::load(&state_path).is_err());
}
