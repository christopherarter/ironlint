//! Origin tracking on the post-extends merge. The walker must attribute
//! every rule to the file it was defined in, with local definitions winning
//! on collision (matching `resolve`'s merge semantics).

use hector_core::config::extends::resolve_with_origin;
use std::path::PathBuf;
use tempfile::tempdir;

fn write(p: &std::path::Path, body: &str) {
    std::fs::write(p, body).unwrap();
}

#[test]
fn origin_map_attributes_each_rule_to_its_defining_file() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "schema_version: 2\nrules:\n  inherited:\n    description: \"from parent\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: warning\n    script: \"true\"\n  overridden:\n    description: \"parent version\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    );
    let child = dir.path().join(".hector.yml");
    write(
        &child,
        "schema_version: 2\nextends: [\"parent.yml\"]\nrules:\n  local:\n    description: \"only in child\"\n    engine: script\n    scope: [\"*.md\"]\n    severity: warning\n    script: \"true\"\n  overridden:\n    description: \"child wins\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: warning\n    script: \"true\"\n",
    );

    let (cfg, origins) = resolve_with_origin(&child).unwrap();

    assert_eq!(cfg.rules.len(), 3, "merged rule count");
    assert_eq!(
        cfg.rules.get("overridden").unwrap().description,
        "child wins",
        "local wins on collision"
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
        "schema_version: 2\nrules:\n  from-grand:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: warning\n    script: \"true\"\n",
    );
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "schema_version: 2\nextends: [\"grand.yml\"]\nrules: {}\n",
    );
    let child = dir.path().join(".hector.yml");
    write(
        &child,
        "schema_version: 2\nextends: [\"parent.yml\"]\nrules: {}\n",
    );

    let (cfg, origins) = resolve_with_origin(&child).unwrap();
    assert_eq!(cfg.rules.len(), 1);
    assert_eq!(
        origins.get("from-grand").unwrap(),
        &grand.canonicalize().unwrap()
    );
}
