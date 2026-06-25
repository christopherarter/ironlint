//! `extends::resolve_paths` returns the canonical closure of config files used
//! by the trust layer. It must include the root plus every transitively
//! extended file, deduped and deterministically sorted, and reuse the same
//! cycle detection as `resolve`.

use hector_core::config::extends::resolve_paths;
use std::path::Path;
use tempfile::tempdir;

fn write(p: &Path, body: &str) {
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, body).unwrap();
}

fn canon(p: &Path) -> std::path::PathBuf {
    p.canonicalize().unwrap()
}

#[test]
fn no_extends_returns_just_the_root() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    write(&cfg, "gates:\n  g:\n    files: \"*\"\n    run: \"true\"\n");

    let paths = resolve_paths(&cfg).unwrap();
    assert_eq!(paths, vec![canon(&cfg)]);
}

#[test]
fn chain_returns_every_file_sorted_and_deduped() {
    let dir = tempdir().unwrap();
    let grand = dir.path().join("grand.yml");
    write(
        &grand,
        "gates:\n  g:\n    files: \"*\"\n    run: \"true\"\n",
    );
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "extends: [\"grand.yml\"]\ngates:\n  p:\n    files: \"*\"\n    run: \"true\"\n",
    );
    let child = dir.path().join(".hector.yml");
    write(&child, "extends: [\"parent.yml\"]\ngates: {}\n");

    let paths = resolve_paths(&child).unwrap();
    let mut want = vec![canon(&grand), canon(&parent), canon(&child)];
    want.sort();
    assert_eq!(paths, want, "closure is the deterministically-sorted set");
}

#[test]
fn diamond_dedupes_the_shared_base() {
    // child extends both left and right; both extend the same base. The base
    // must appear exactly once.
    let dir = tempdir().unwrap();
    let base = dir.path().join("base.yml");
    write(&base, "gates:\n  b:\n    files: \"*\"\n    run: \"true\"\n");
    let left = dir.path().join("left.yml");
    write(&left, "extends: [\"base.yml\"]\ngates: {}\n");
    let right = dir.path().join("right.yml");
    write(&right, "extends: [\"base.yml\"]\ngates: {}\n");
    let child = dir.path().join(".hector.yml");
    write(
        &child,
        "extends: [\"left.yml\", \"right.yml\"]\ngates: {}\n",
    );

    let paths = resolve_paths(&child).unwrap();
    let base_canon = canon(&base);
    assert_eq!(
        paths.iter().filter(|p| **p == base_canon).count(),
        1,
        "diamond base folded once: {paths:?}"
    );
    assert_eq!(paths.len(), 4, "child + left + right + base: {paths:?}");
}

#[test]
fn cycle_is_an_error() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a.yml");
    let b = dir.path().join("b.yml");
    write(&a, "extends: [./b.yml]\ngates: {}\n");
    write(&b, "extends: [./a.yml]\ngates: {}\n");

    let err = resolve_paths(&a).unwrap_err().to_string().to_lowercase();
    assert!(err.contains("cycle"), "cycle must be reported: {err}");
}

#[test]
fn missing_extends_target_is_an_error() {
    let dir = tempdir().unwrap();
    let child = dir.path().join(".hector.yml");
    write(&child, "extends: [\"./nope.yml\"]\ngates: {}\n");

    assert!(resolve_paths(&child).is_err());
}
