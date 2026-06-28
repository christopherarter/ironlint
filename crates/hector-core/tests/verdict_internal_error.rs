use hector_core::verdict::{Block, GateError, Status, Verdict};

#[test]
fn errors_only_is_internal_error() {
    let v = Verdict::from_outcomes(
        vec![],
        vec![GateError {
            check: "g".to_string(),
            step: None,
            file: Some("f".to_string()),
            reason: "not_found".to_string(),
        }],
        vec![],
        0,
    );
    assert_eq!(v.status, Status::InternalError);
}

#[test]
fn block_plus_error_is_block() {
    // A confirmed policy violation (exit 2) must not be downgraded to
    // fail-open by an unrelated check crash — Block wins over InternalError.
    let v = Verdict::from_outcomes(
        vec![Block {
            check: "gate-a".to_string(),
            step: None,
            file: Some("a.rs".to_string()),
            message: "blocked".to_string(),
        }],
        vec![GateError {
            check: "gate-b".to_string(),
            step: None,
            file: Some("a.rs".to_string()),
            reason: "timeout".to_string(),
        }],
        vec![],
        0,
    );
    assert_eq!(v.status, Status::Block);
}
