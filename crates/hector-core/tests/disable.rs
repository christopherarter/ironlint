use hector_core::disable::DisableMap;

const SOURCE: &str = "\
let x = 1;
eval(expr); // hector-disable: no-eval reason: sandboxed input
console.log('hi'); /* hector-disable: no-console-log reason: debug only */
let y = 2;
";

#[test]
fn detects_line_comment_disable() {
    let map = DisableMap::from_source(SOURCE);
    assert!(map.is_disabled(2, "no-eval"));
    assert!(!map.is_disabled(2, "no-console-log"));
}

#[test]
fn detects_block_comment_disable() {
    let map = DisableMap::from_source(SOURCE);
    assert!(map.is_disabled(3, "no-console-log"));
}

#[test]
fn returns_false_when_no_disable() {
    let map = DisableMap::from_source(SOURCE);
    assert!(!map.is_disabled(1, "no-eval"));
    assert!(!map.is_disabled(4, "no-console-log"));
}

#[test]
fn parses_comma_separated_rule_list() {
    let src = "let x = 1; // hector-disable: a, b reason: x\n";
    let map = DisableMap::from_source(src);
    assert!(map.is_disabled(1, "a"));
    assert!(map.is_disabled(1, "b"));
    assert!(!map.is_disabled(1, "reason"));
    assert!(!map.is_disabled(1, "reason:"));
    assert!(!map.is_disabled(1, "x"));
}

#[test]
fn trims_trailing_comma_from_rule_id() {
    let src = "let x = 1; // hector-disable: a, reason: x\n";
    let map = DisableMap::from_source(src);
    assert!(map.is_disabled(1, "a"));
    assert!(!map.is_disabled(1, "a,"));
    assert!(!map.is_disabled(1, "reason"));
}

#[test]
fn existing_single_rule_unchanged() {
    let src = "eval(expr); // hector-disable: no-eval reason: x\n";
    let map = DisableMap::from_source(src);
    assert!(map.is_disabled(1, "no-eval"));
    assert!(!map.is_disabled(1, "reason"));
    assert!(!map.is_disabled(1, "x"));
}
