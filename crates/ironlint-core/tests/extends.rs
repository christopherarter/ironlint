use ironlint_core::config::parse_file_with_extends;
use tempfile::tempdir;

fn write(p: &std::path::Path, body: &str) {
    std::fs::write(p, body).unwrap();
}

#[test]
fn extends_inherits_gates_from_parent() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "checks:\n  base-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let child = dir.path().join(".ironlint.yml");
    write(
        &child,
        "extends: [\"parent.yml\"]\nchecks:\n  child-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let cfg = parse_file_with_extends(&child).expect("parse");
    assert!(
        cfg.checks.contains_key("base-gate"),
        "inherited check present"
    );
    assert!(cfg.checks.contains_key("child-gate"), "local check present");
}

#[test]
fn extends_local_gate_wins_on_collision() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "checks:\n  shared:\n    files: \"**/*.rs\"\n    run: \"echo parent\"\n",
    );
    let child = dir.path().join(".ironlint.yml");
    write(
        &child,
        "extends: [\"parent.yml\"]\nchecks:\n  shared:\n    files: \"**/*.ts\"\n    run: \"echo child\"\n",
    );
    let cfg = parse_file_with_extends(&child).expect("parse");
    assert_eq!(
        cfg.checks["shared"].run,
        Some("echo child".to_string()),
        "local check wins"
    );
}

#[test]
fn extends_inherits_execution_timeout_from_parent() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "execution:\n  timeout_secs: 120\nchecks:\n  base-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let child = dir.path().join(".ironlint.yml");
    write(
        &child,
        "extends: [\"parent.yml\"]\nchecks:\n  child-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let cfg = parse_file_with_extends(&child).expect("parse");
    assert_eq!(
        cfg.timeout_secs(),
        120,
        "child with no execution block inherits the base's timeout"
    );
}

#[test]
fn extends_local_execution_timeout_wins_over_parent() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "execution:\n  timeout_secs: 120\nchecks:\n  base-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let child = dir.path().join(".ironlint.yml");
    write(
        &child,
        "extends: [\"parent.yml\"]\nexecution:\n  timeout_secs: 5\nchecks:\n  child-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let cfg = parse_file_with_extends(&child).expect("parse");
    assert_eq!(
        cfg.timeout_secs(),
        5,
        "child's explicit timeout overrides the inherited one"
    );
}

#[test]
fn extends_no_execution_anywhere_defaults_to_30() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "checks:\n  base-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let child = dir.path().join(".ironlint.yml");
    write(
        &child,
        "extends: [\"parent.yml\"]\nchecks:\n  child-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let cfg = parse_file_with_extends(&child).expect("parse");
    assert_eq!(
        cfg.timeout_secs(),
        30,
        "neither config sets execution — resolved timeout is still the default"
    );
}

#[test]
fn extends_chain_three_levels() {
    let dir = tempdir().unwrap();
    let grand = dir.path().join("grand.yml");
    write(
        &grand,
        "checks:\n  from-grand:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "extends: [\"grand.yml\"]\nchecks:\n  from-parent:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let child = dir.path().join(".ironlint.yml");
    write(&child, "extends: [\"parent.yml\"]\nchecks: {}\n");
    let cfg = parse_file_with_extends(&child).expect("parse");
    assert!(cfg.checks.contains_key("from-grand"));
    assert!(cfg.checks.contains_key("from-parent"));
}

#[test]
fn cycle_in_extends_is_error() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a.yml");
    let b = dir.path().join("b.yml");
    write(&a, "extends: [./b.yml]\nchecks: {}\n");
    write(&b, "extends: [./a.yml]\nchecks: {}\n");
    let result = parse_file_with_extends(&a);
    assert!(result.is_err(), "cycle detection should fail");
    let err = format!("{:#}", result.unwrap_err()).to_lowercase();
    assert!(
        err.contains("cycle") || err.contains("loop"),
        "error mentions cycle: {err}"
    );
}

#[test]
fn extends_errors_for_nonexistent_parent() {
    let dir = tempdir().unwrap();
    let child = dir.path().join("child.yml");
    write(&child, "extends: [\"./does-not-exist.yml\"]\nchecks: {}\n");
    let err = parse_file_with_extends(&child).expect_err("missing parent");
    let msg = format!("{err:#}").to_lowercase();
    assert!(msg.contains("canonicaliz") || msg.contains("no such file"));
}
