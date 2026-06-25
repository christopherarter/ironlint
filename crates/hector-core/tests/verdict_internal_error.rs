use hector_core::verdict::{Block, GateError, Status, Verdict};

#[test]
fn errors_only_is_internal_error() {
    let v = Verdict::from_outcomes(
        vec![],
        vec![GateError {
            gate: "g".to_string(),
            file: "f".to_string(),
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
    // fail-open by an unrelated gate crash — Block wins over InternalError.
    let v = Verdict::from_outcomes(
        vec![Block {
            gate: "gate-a".to_string(),
            file: "a.rs".to_string(),
            message: "blocked".to_string(),
        }],
        vec![GateError {
            gate: "gate-b".to_string(),
            file: "a.rs".to_string(),
            reason: "timeout".to_string(),
        }],
        vec![],
        0,
    );
    assert_eq!(v.status, Status::Block);
}
