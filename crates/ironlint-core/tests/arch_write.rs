use ironlint_core::arch::{config::ArchConfig, evaluate::evaluate_outgoing, graph::DepGraph};
use std::fs;

fn base_yaml(may_import_data: bool, ignore: &[&str]) -> String {
    let may_import = if may_import_data { "[data]" } else { "[]" };
    let ignore = if ignore.is_empty() {
        String::new()
    } else {
        format!("ignore: {ignore:?}\n")
    };
    format!(
        "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\nrules:\n  - from: presentation\n    may_import: {may_import}\n{ignore}"
    )
}

fn config(may_import_data: bool) -> ArchConfig {
    serde_yaml::from_str(&base_yaml(may_import_data, &[])).unwrap()
}

fn config_with_ignore(may_import_data: bool) -> ArchConfig {
    serde_yaml::from_str(&base_yaml(may_import_data, &["**/*.test.ts"])).unwrap()
}

#[test]
fn proposed_forbidden_import_blocks_but_allowed_import_passes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    fs::write(root.join("src/data/db.ts"), "export const db = 1;\n").unwrap();
    let proposed = root.join("src/components/App.tsx");
    let content = b"import { db } from '../data/db';\n";

    let graph = DepGraph::build(root, &config(false)).unwrap();
    assert_eq!(
        evaluate_outgoing(content, &proposed, root, &graph, &config(false))
            .unwrap()
            .len(),
        1
    );

    let graph = DepGraph::build(root, &config(true)).unwrap();
    assert!(
        evaluate_outgoing(content, &proposed, root, &graph, &config(true))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn next_write_rebuilds_from_the_prior_accepted_write_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    let cfg = config(true);

    let first = root.join("src/data/new.ts");
    fs::write(&first, "export const newValue = 1;\n").unwrap();

    let graph = DepGraph::build(root, &cfg).unwrap();
    let second = root.join("src/components/App.tsx");
    assert!(evaluate_outgoing(
        b"import { newValue } from '../data/new';\n",
        &second,
        root,
        &graph,
        &cfg
    )
    .unwrap()
    .is_empty());
}

#[test]
fn ignored_file_not_blocked_on_write() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    fs::write(root.join("src/data/db.ts"), "export const db = 1;\n").unwrap();

    let cfg = config_with_ignore(false);
    let graph = DepGraph::build(root, &cfg).unwrap();
    let proposed = root.join("src/components/App.test.ts");
    let content = b"import { db } from '../data/db';\n";

    assert!(
        evaluate_outgoing(content, &proposed, root, &graph, &cfg)
            .unwrap()
            .is_empty(),
        "ignored files must not be subject to architecture rules on write"
    );
}

#[test]
fn non_ignored_file_still_blocked_on_write() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    fs::write(root.join("src/data/db.ts"), "export const db = 1;\n").unwrap();

    let cfg = config_with_ignore(false);
    let graph = DepGraph::build(root, &cfg).unwrap();
    let proposed = root.join("src/components/App.tsx");
    let content = b"import { db } from '../data/db';\n";

    assert_eq!(
        evaluate_outgoing(content, &proposed, root, &graph, &cfg)
            .unwrap()
            .len(),
        1,
        "non-ignored files must still be blocked for forbidden imports"
    );
}
