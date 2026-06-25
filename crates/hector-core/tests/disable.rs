use hector_core::disable::is_disabled;

#[test]
fn disables_named_gate() {
    assert!(is_disabled("code // hector-disable: no-todo\n", "no-todo"));
    assert!(!is_disabled("code // hector-disable: other\n", "no-todo"));
}

#[test]
fn preserves_namespaced_gate_ids() {
    assert!(is_disabled(
        "x // hector-disable: python/no-print\n",
        "python/no-print"
    ));
    assert!(!is_disabled(
        "x // hector-disable: python/no-print\n",
        "python"
    ));
}

#[test]
fn stops_at_block_comment_close() {
    assert!(is_disabled("x /* hector-disable: g1 */\n", "g1"));
}

#[test]
fn stops_at_reason_keyword() {
    assert!(is_disabled(
        "x // hector-disable: g1 reason: legacy\n",
        "g1"
    ));
    assert!(!is_disabled(
        "x // hector-disable: g1 reason: legacy\n",
        "reason:"
    ));
}

#[test]
fn comma_and_whitespace_separated() {
    let content = "// hector-disable: a, b c\n";
    assert!(is_disabled(content, "a"));
    assert!(is_disabled(content, "b"));
    assert!(is_disabled(content, "c"));
}
