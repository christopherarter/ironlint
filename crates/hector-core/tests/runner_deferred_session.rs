use hector_core::runner::{CheckOptions, HectorEngine};
use hector_core::session_state::{EditRecord, SessionState};
use std::collections::HashSet;
use std::fs;
use tempfile::tempdir;

const CFG: &str = r#"
schema_version: 2
trust:
  fingerprint: PLACEHOLDER
llm:
  provider: claude-code-subagent
rules:
  cross-edit-check:
    description: aggregate review across the session
    engine: session
    scope: ["src/**"]
    severity: error
"#;

fn write_cfg(dir: &std::path::Path) -> std::path::PathBuf {
    let p = dir.join(".hector.yml");
    fs::write(&p, CFG).unwrap();
    let signed = hector_core::trust::write_trust_block(&fs::read_to_string(&p).unwrap()).unwrap();
    fs::write(&p, signed).unwrap();
    p
}

#[test]
fn subagent_session_stop_emits_deferred_envelope() {
    let tmp = tempdir().unwrap();
    let cfg = write_cfg(tmp.path());
    // Create the files so canonicalize() succeeds for scope-matching.
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    let file_a = tmp.path().join("src/a.rs");
    let file_b = tmp.path().join("src/b.rs");
    fs::write(&file_a, "let MARKER_A = 1;\n").unwrap();
    fs::write(&file_b, "let MARKER_B = 2;\n").unwrap();
    let engine = HectorEngine::builder()
        .with_options(CheckOptions {
            rules: HashSet::new(),
            explain: false,
            emit_semantic_payload: true,
            allow_external_paths: false,
        })
        .load(&cfg)
        .unwrap();
    let state = SessionState {
        session_id: "s-test".into(),
        started_at: "2026-05-26T00:00:00Z".into(),
        edits: vec![
            EditRecord {
                file: file_a.to_string_lossy().into(),
                diff: "+let MARKER_A = 1;\n".into(),
                timestamp: "2026-05-26T00:00:01Z".into(),
            },
            EditRecord {
                file: file_b.to_string_lossy().into(),
                diff: "+let MARKER_B = 2;\n".into(),
                timestamp: "2026-05-26T00:00:02Z".into(),
            },
        ],
    };
    let report = engine.check_session_with_options(&state).expect("ok");
    let deferred = report.deferred.expect("deferred envelope for session stop");
    assert_eq!(deferred.payload.evaluate.len(), 1);
    assert_eq!(deferred.payload.evaluate[0].id, "cross-edit-check");
    assert!(
        deferred.payload.diff.contains("src/a.rs") && deferred.payload.diff.contains("MARKER_A"),
        "session-aggregate framing must reference each edit and its diff"
    );
    assert!(
        deferred.payload.diff.contains("src/b.rs") && deferred.payload.diff.contains("MARKER_B"),
    );
    assert_eq!(
        deferred.payload.file, "",
        "session-level deferred envelope has empty `file`"
    );
}

#[test]
fn subagent_session_with_no_in_scope_rules_returns_pass_no_envelope() {
    // When no session rule matches any edit, the CheckReport should be
    // a clean pass — not a deferred envelope. Mirrors the per-file
    // semantic of "no rule → no envelope."
    let tmp = tempdir().unwrap();
    let cfg = write_cfg(tmp.path());
    let engine = HectorEngine::builder()
        .with_options(CheckOptions {
            rules: HashSet::new(),
            explain: false,
            emit_semantic_payload: true,
            allow_external_paths: false,
        })
        .load(&cfg)
        .unwrap();
    let state = SessionState {
        session_id: "s-empty".into(),
        started_at: "2026-05-26T00:00:00Z".into(),
        edits: vec![EditRecord {
            file: tmp.path().join("docs/readme.md").to_string_lossy().into(), // outside src/**
            diff: "+text\n".into(),
            timestamp: "2026-05-26T00:00:01Z".into(),
        }],
    };
    let report = engine.check_session_with_options(&state).expect("ok");
    assert!(
        report.deferred.is_none(),
        "no session rule in scope → no envelope"
    );
}
