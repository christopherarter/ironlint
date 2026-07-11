use ironlint_core::arch::config::ArchConfig;
use ironlint_core::arch::engine::{ArchEngine, ArchOutcome};
use std::fs;
use std::path::Path;

fn forbidden_config() -> ArchConfig {
    serde_yaml::from_str(
        "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\nrules:\n  - from: presentation\n    may_import: []\n",
    )
    .unwrap()
}

fn permitted_config() -> ArchConfig {
    serde_yaml::from_str(
        "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\nrules:\n  - from: presentation\n    may_import: [data]\n",
    )
    .unwrap()
}

fn make_ts_repo(root: &Path) {
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    fs::write(
        root.join("src/components/App.tsx"),
        "import { db } from '../data/db';\n",
    )
    .unwrap();
    fs::write(root.join("src/data/db.ts"), "export const db = 1;\n").unwrap();
}

#[test]
fn check_whole_blocks_forbidden_edge() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    make_ts_repo(root);

    let outcome = ArchEngine::check_whole(root, &forbidden_config());
    match outcome {
        ArchOutcome::Block { violations } => {
            assert_eq!(violations.len(), 1);
            assert_eq!(violations[0].rule_from, "presentation");
            assert_eq!(violations[0].importer, root.join("src/components/App.tsx"));
            assert_eq!(violations[0].target, root.join("src/data/db.ts"));
        }
        other => panic!("expected Block, got {other:?}"),
    }
}

#[test]
fn check_whole_passes_permitted_edge() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    make_ts_repo(root);

    let outcome = ArchEngine::check_whole(root, &permitted_config());
    assert!(matches!(outcome, ArchOutcome::Pass), "{outcome:?}");
}

#[test]
fn check_whole_internal_error_when_root_missing() {
    let outcome =
        ArchEngine::check_whole(Path::new("/does/not/exist/arch-root"), &forbidden_config());
    assert!(
        matches!(outcome, ArchOutcome::InternalError(_)),
        "{outcome:?}"
    );
}

#[test]
fn check_write_blocks_proposed_forbidden_import() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    make_ts_repo(root);

    let proposed = root.join("src/components/App.tsx");
    let content = b"import { db } from '../data/db';\n";
    let outcome = ArchEngine::check_write(root, &forbidden_config(), &proposed, content, None);
    match outcome {
        ArchOutcome::Block { violations } => {
            assert_eq!(violations.len(), 1);
            assert_eq!(violations[0].rule_from, "presentation");
        }
        other => panic!("expected Block, got {other:?}"),
    }
}

#[test]
fn check_write_passes_when_no_forbidden_import() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    make_ts_repo(root);

    let proposed = root.join("src/components/App.tsx");
    let content = b"import { local } from './local';\n";
    let outcome = ArchEngine::check_write(root, &forbidden_config(), &proposed, content, None);
    assert!(matches!(outcome, ArchOutcome::Pass), "{outcome:?}");
}

#[test]
fn check_write_internal_error_when_root_missing() {
    let proposed = Path::new("/does/not/exist/arch-root/src/components/App.tsx");
    let outcome = ArchEngine::check_write(
        Path::new("/does/not/exist/arch-root"),
        &forbidden_config(),
        proposed,
        b"import { db } from '../data/db';\n",
        None,
    );
    assert!(
        matches!(outcome, ArchOutcome::InternalError(_)),
        "{outcome:?}"
    );
}

#[test]
fn graph_returns_nodes_for_valid_repo() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    make_ts_repo(root);

    let graph = ArchEngine::graph(root, &forbidden_config()).unwrap();
    assert!(graph
        .nodes
        .contains_key(&root.join("src/components/App.tsx")));
    assert!(graph.nodes.contains_key(&root.join("src/data/db.ts")));
}

#[test]
fn graph_returns_error_when_root_missing() {
    let result = ArchEngine::graph(Path::new("/does/not/exist/arch-root"), &forbidden_config());
    assert!(result.is_err());
}

#[test]
fn why_returns_importer_violations() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    make_ts_repo(&root);

    let violations = ArchEngine::why(
        &root,
        &forbidden_config(),
        Path::new("src/components/App.tsx"),
    )
    .unwrap();
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].importer, root.join("src/components/App.tsx"));
    assert_eq!(violations[0].rule_from, "presentation");
}

#[test]
fn why_returns_empty_for_unrelated_importer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    make_ts_repo(root);

    let violations =
        ArchEngine::why(root, &forbidden_config(), Path::new("src/data/db.ts")).unwrap();
    assert!(violations.is_empty());
}

#[test]
fn why_returns_violations_for_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    make_ts_repo(&root);

    // Pass an ABSOLUTE path — exercises the `path.is_absolute()` → true arm
    // in `ArchEngine::why` that was previously untested (existing tests use
    // a relative path like "src/components/App.tsx").
    let violations = ArchEngine::why(
        &root,
        &forbidden_config(),
        &root.join("src/components/App.tsx"),
    )
    .unwrap();
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].importer, root.join("src/components/App.tsx"));
    assert_eq!(violations[0].rule_from, "presentation");
}

#[cfg(unix)]
#[test]
fn why_finds_violations_via_symlinked_path() {
    // Graph keys are canonical (they come from walking `root`), so `why`
    // must canonicalize the requested path before comparing — otherwise a
    // caller passing a symlinked path gets an empty result and a real
    // violation is missed (Bug 10). The previous version of this test leaned
    // on the macOS-only `/tmp` -> `/private/tmp` symlink, which left the
    // canonicalization with ZERO coverage on Linux CI: a refactor dropping
    // `canonicalize_through_parent(path)` would stay green on Linux and red
    // only on macOS. This portable version creates its own symlink so the
    // same code path runs on every Unix — Linux and macOS alike.
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    // Canonicalize root up front so `canon` below matches the graph's keys
    // (which are canonical by construction). On macOS this resolves the
    // `/var` -> `/private/var` indirection; on Linux it is usually a no-op.
    let root = dir.path().canonicalize().unwrap();
    make_ts_repo(&root);

    // Canonical importer the graph will key on.
    let canon = root.join("src/components/App.tsx");
    // A symlinked alias to `root`. Passing a path through it reproduces the
    // canonical/non-canonical mismatch Bug 10 guards: the graph holds `canon`,
    // the request arrives as `alias/...`. Without canonicalization the two
    // don't compare equal and the violation is dropped.
    let alias_dir = dir.path().join("alias");
    symlink(&root, &alias_dir).unwrap();
    let alias = alias_dir.join("src/components/App.tsx");

    let violations = ArchEngine::why(&root, &forbidden_config(), &alias).unwrap();
    assert_eq!(
        violations.len(),
        1,
        "should find violation via non-canonical (symlinked) path"
    );
    assert_eq!(violations[0].importer, canon);
    assert_eq!(violations[0].rule_from, "presentation");
}

#[test]
fn why_returns_error_when_root_missing() {
    let result = ArchEngine::why(
        Path::new("/does/not/exist/arch-root"),
        &forbidden_config(),
        Path::new("src/components/App.tsx"),
    );
    assert!(result.is_err());
}

// --- CommonJS require() (Bug 2) ---
//
// require() imports must be extracted so a forbidden .cjs import cannot sneak
// through. Both the whole-graph sweep and the per-write check must see them.

fn make_cjs_repo(root: &Path) {
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    fs::write(
        root.join("src/components/App.cjs"),
        "const db = require('../data/db');\n",
    )
    .unwrap();
    fs::write(
        root.join("src/data/db.cjs"),
        "module.exports = { db: 1 };\n",
    )
    .unwrap();
}

#[test]
fn check_whole_blocks_forbidden_cjs_require() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    make_cjs_repo(root);

    let outcome = ArchEngine::check_whole(root, &forbidden_config());
    match outcome {
        ArchOutcome::Block { violations } => {
            assert_eq!(violations.len(), 1);
            assert_eq!(violations[0].rule_from, "presentation");
            assert_eq!(violations[0].importer, root.join("src/components/App.cjs"));
            assert_eq!(violations[0].target, root.join("src/data/db.cjs"));
        }
        other => panic!("expected Block, got {other:?}"),
    }
}

#[test]
fn check_write_blocks_proposed_forbidden_cjs_require() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    make_cjs_repo(root);

    let proposed = root.join("src/components/App.cjs");
    let content = b"const db = require('../data/db');\n";
    let outcome = ArchEngine::check_write(root, &forbidden_config(), &proposed, content, None);
    match outcome {
        ArchOutcome::Block { violations } => {
            assert_eq!(violations.len(), 1);
            assert_eq!(violations[0].rule_from, "presentation");
        }
        other => panic!("expected Block, got {other:?}"),
    }
}
