//! Origin tracking on the post-extends merge. The walker must attribute
//! every gate to the file it was defined in, with local definitions winning
//! on collision (matching `resolve`'s merge semantics).

use hector_core::config::extends::resolve_with_origin;
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
        "gates:\n  inherited:\n    files: \"**/*.txt\"\n    run: \"exit 0\"\n  overridden:\n    files: \"**/*.txt\"\n    run: \"exit 0\"\n",
    );
    let child = dir.path().join(".hector.yml");
    write(
        &child,
        "extends: [\"parent.yml\"]\ngates:\n  local:\n    files: \"**/*.md\"\n    run: \"exit 0\"\n  overridden:\n    files: \"**/*.ts\"\n    run: \"exit 0\"\n",
    );

    let (cfg, origins) = resolve_with_origin(&child).unwrap();

    assert_eq!(cfg.gates.len(), 3, "merged gate count");
    assert_eq!(
        cfg.gates["overridden"].run, "exit 0",
        "local gate wins on collision"
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
        "gates:\n  from-grand:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let parent = dir.path().join("parent.yml");
    write(&parent, "extends: [\"grand.yml\"]\ngates: {}\n");
    let child = dir.path().join(".hector.yml");
    write(&child, "extends: [\"parent.yml\"]\ngates: {}\n");

    let (cfg, origins) = resolve_with_origin(&child).unwrap();
    assert_eq!(cfg.gates.len(), 1);
    assert_eq!(
        origins.get("from-grand").unwrap(),
        &grand.canonicalize().unwrap()
    );
}
