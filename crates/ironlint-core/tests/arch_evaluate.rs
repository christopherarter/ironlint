use ironlint_core::arch::config::ArchConfig;
use ironlint_core::arch::evaluate::evaluate;
use ironlint_core::arch::graph::DepGraph;
use std::fs;

#[test]
fn flags_forbidden_edge() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    fs::write(root.join("src/data/db.ts"), "export const db = 1;\n").unwrap();
    fs::write(
        root.join("src/components/App.tsx"),
        "import { db } from '../data/db';\n",
    )
    .unwrap();
    let config: ArchConfig = serde_yaml::from_str(
        "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\nrules:\n  - from: presentation\n    may_import: []\n",
    ).unwrap();
    let graph = DepGraph::build(root, &config).unwrap();
    let violations = evaluate(&graph, &config);
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].rule_from, "presentation");
}

#[test]
fn allows_permitted_edge() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    fs::write(root.join("src/data/db.ts"), "export const db = 1;\n").unwrap();
    fs::write(
        root.join("src/components/App.tsx"),
        "import { db } from '../data/db';\n",
    )
    .unwrap();
    let config: ArchConfig = serde_yaml::from_str(
        "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\nrules:\n  - from: presentation\n    may_import: [data]\n",
    ).unwrap();
    let graph = DepGraph::build(root, &config).unwrap();
    let violations = evaluate(&graph, &config);
    assert!(violations.is_empty(), "{violations:?}");
}
