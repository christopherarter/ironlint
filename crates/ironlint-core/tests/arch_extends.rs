use ironlint_core::config::parse_file_with_extends;
use tempfile::tempdir;

fn write(p: &std::path::Path, body: &str) {
    std::fs::write(p, body).unwrap();
}

#[test]
fn extends_inherits_parent_architecture_when_child_omits_it() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "architecture:\n  layers:\n    - name: data\n      globs: [\"src/data/**\"]\n    - name: presentation\n      globs: [\"src/ui/**\"]\n  rules:\n    - from: data\n      may_import: []\nchecks:\n  base-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let child = dir.path().join(".ironlint.yml");
    write(
        &child,
        "extends: [\"parent.yml\"]\nchecks:\n  child-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let cfg = parse_file_with_extends(&child).expect("parse");
    let arch = cfg.architecture.expect("msg");
    assert_eq!(arch.layers.len(), 2);
    assert_eq!(arch.layers[0].name, "data");
    assert_eq!(arch.layers[1].name, "presentation");
    assert_eq!(arch.rules.len(), 1);
    assert_eq!(arch.rules[0].from, "data");
}

#[test]
fn extends_local_architecture_replaces_parent() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "architecture:\n  layers:\n    - name: data\n      globs: [\"src/data/**\"]\n    - name: presentation\n      globs: [\"src/ui/**\"]\n  rules:\n    - from: data\n      may_import: []\nchecks:\n  base-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let child = dir.path().join(".ironlint.yml");
    write(
        &child,
        "extends: [\"parent.yml\"]\narchitecture:\n  layers:\n    - name: api\n      globs: [\"src/api/**\"]\n  rules:\n    - from: api\n      may_import: [api]\nchecks:\n  child-gate:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let cfg = parse_file_with_extends(&child).expect("parse");
    let arch = cfg.architecture.expect("msg");
    assert_eq!(arch.layers.len(), 1);
    assert_eq!(arch.layers[0].name, "api");
    assert_eq!(arch.rules.len(), 1);
    assert_eq!(arch.rules[0].from, "api");
}
