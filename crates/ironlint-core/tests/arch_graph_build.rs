use ironlint_core::arch::config::ArchConfig;
use ironlint_core::arch::graph::DepGraph;
use std::fs;

#[test]
fn builds_graph_from_ts_repo() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    fs::write(
        root.join("src/components/App.tsx"),
        "import { db } from '../data/db';\n",
    )
    .unwrap();
    fs::write(root.join("src/data/db.ts"), "export const db = 1;\n").unwrap();

    let config: ArchConfig = serde_yaml::from_str(
        "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\n",
    )
    .unwrap();
    let graph = DepGraph::build(root, &config).unwrap();
    let app = root.join("src/components/App.tsx");
    let node = graph.nodes.get(&app).expect("App node exists");
    assert_eq!(node.edges.len(), 1);
    assert_eq!(node.edges[0].target, root.join("src/data/db.ts"));
}
