use hector_core::config::parse_file_with_extends;
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
        "gates:\n  base-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let child = dir.path().join(".hector.yml");
    write(
        &child,
        "extends: [\"parent.yml\"]\ngates:\n  child-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let cfg = parse_file_with_extends(&child).expect("parse");
    assert!(
        cfg.gates.contains_key("base-gate"),
        "inherited gate present"
    );
    assert!(cfg.gates.contains_key("child-gate"), "local gate present");
}

#[test]
fn extends_local_gate_wins_on_collision() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "gates:\n  shared:\n    files: \"**/*.rs\"\n    run: \"echo parent\"\n",
    );
    let child = dir.path().join(".hector.yml");
    write(
        &child,
        "extends: [\"parent.yml\"]\ngates:\n  shared:\n    files: \"**/*.ts\"\n    run: \"echo child\"\n",
    );
    let cfg = parse_file_with_extends(&child).expect("parse");
    assert_eq!(cfg.gates["shared"].run, "echo child", "local gate wins");
}

#[test]
fn extends_chain_three_levels() {
    let dir = tempdir().unwrap();
    let grand = dir.path().join("grand.yml");
    write(
        &grand,
        "gates:\n  from-grand:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "extends: [\"grand.yml\"]\ngates:\n  from-parent:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let child = dir.path().join(".hector.yml");
    write(&child, "extends: [\"parent.yml\"]\ngates: {}\n");
    let cfg = parse_file_with_extends(&child).expect("parse");
    assert!(cfg.gates.contains_key("from-grand"));
    assert!(cfg.gates.contains_key("from-parent"));
}

#[test]
fn cycle_in_extends_is_error() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a.yml");
    let b = dir.path().join("b.yml");
    write(&a, "extends: [./b.yml]\ngates: {}\n");
    write(&b, "extends: [./a.yml]\ngates: {}\n");
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
    write(&child, "extends: [\"./does-not-exist.yml\"]\ngates: {}\n");
    let err = parse_file_with_extends(&child).expect_err("missing parent");
    let msg = format!("{err:#}").to_lowercase();
    assert!(msg.contains("canonicaliz") || msg.contains("no such file"));
}
