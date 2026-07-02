//! Origin tracking on the post-extends merge. The walker must attribute
//! every check to the file it was defined in, with local definitions winning
//! on collision (matching `resolve`'s merge semantics).

use ironlint_core::config::extends::resolve_with_origin;
use std::path::PathBuf;
use tempfile::tempdir;

fn write(p: &std::path::Path, body: &str) {
    std::fs::write(p, body).unwrap();
}

#[test]
fn origin_map_attributes_each_gate_to_its_defining_file() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "checks:\n  inherited:\n    files: \"**/*.txt\"\n    run: \"exit 0\"\n  overridden:\n    files: \"**/*.txt\"\n    run: \"exit 0\"\n",
    );
    let child = dir.path().join(".ironlint.yml");
    write(
        &child,
        "extends: [\"parent.yml\"]\nchecks:\n  local:\n    files: \"**/*.md\"\n    run: \"exit 0\"\n  overridden:\n    files: \"**/*.ts\"\n    run: \"exit 0\"\n",
    );

    let (cfg, origins) = resolve_with_origin(&child).unwrap();

    assert_eq!(cfg.checks.len(), 3, "merged check count");
    assert_eq!(
        cfg.checks["overridden"].run,
        Some("exit 0".to_string()),
        "local check wins on collision"
    );

    let canon_child: PathBuf = child.canonicalize().unwrap();
    let canon_parent: PathBuf = parent.canonicalize().unwrap();
    assert_eq!(origins.get("local").unwrap(), &canon_child);
    assert_eq!(
        origins.get("overridden").unwrap(),
        &canon_child,
        "child wins → child is the origin"
    );
    assert_eq!(origins.get("inherited").unwrap(), &canon_parent);
}

#[test]
fn origin_map_records_transitive_grandparent() {
    let dir = tempdir().unwrap();
    let grand = dir.path().join("grand.yml");
    write(
        &grand,
        "checks:\n  from-grand:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let parent = dir.path().join("parent.yml");
    write(&parent, "extends: [\"grand.yml\"]\nchecks: {}\n");
    let child = dir.path().join(".ironlint.yml");
    write(&child, "extends: [\"parent.yml\"]\nchecks: {}\n");

    let (cfg, origins) = resolve_with_origin(&child).unwrap();
    assert_eq!(cfg.checks.len(), 1);
    assert_eq!(
        origins.get("from-grand").unwrap(),
        &grand.canonicalize().unwrap()
    );
}

#[test]
fn origin_resolve_inherits_execution_timeout_from_parent() {
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

    let (cfg, _origins) = resolve_with_origin(&child).unwrap();
    assert_eq!(
        cfg.timeout_secs(),
        120,
        "resolve_with_origin inherits the base's timeout too"
    );
}

#[test]
fn origin_resolve_local_execution_timeout_wins_over_parent() {
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

    let (cfg, _origins) = resolve_with_origin(&child).unwrap();
    assert_eq!(
        cfg.timeout_secs(),
        5,
        "resolve_with_origin: child's explicit timeout overrides the inherited one"
    );
}
