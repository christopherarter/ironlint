//! Linked-worktree trust inheritance, exercised through REAL `git worktree add`
//! metadata — not a hand-crafted directory fixture. These are the acceptance
//! tests for docs/superpowers/specs/2026-07-11-git-worktree-trust-inheritance-design.md.

use ironlint_core::trust::{bless_in, check_trust_in, TrustOutcome};
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Set up a primary git repo with a config + a blocking check script, plus a
/// linked worktree sibling. Returns (primary_cfg, linked_cfg, store_path, script).
fn worktree_pair() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    // Convert the tempdirs to plain paths so the directories survive the
    // helper's return. Use a non-hidden final path component for the worktree
    // itself, because `git worktree add` derives a branch name from it and
    // branch names may not start with `.` (tempfile names do).
    let primary = tempdir().unwrap().keep();
    let linked_root = tempdir().unwrap().keep();
    // git init the primary
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(&primary)
        .status()
        .unwrap()
        .success());
    // write a config + a script that BLOCKS (exits 1) so we can prove check ran
    let cfg = primary.join(".ironlint.yml");
    std::fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \".ironlint/scripts/g.sh\"\n",
    )
    .unwrap();
    let scripts = primary.join(".ironlint/scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    let script = scripts.join("g.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();
    // commit so `git worktree add` has a HEAD
    Command::new("git")
        .args(["add", "."])
        .current_dir(&primary)
        .status()
        .unwrap();
    Command::new("git")
        .args([
            "-c",
            "user.email=t@t.co",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ])
        .current_dir(&primary)
        .status()
        .unwrap();
    // add a linked worktree
    let linked_wt = linked_root.join("wt");
    assert!(Command::new("git")
        .args(["worktree", "add", "-q"])
        .arg(&linked_wt)
        .current_dir(&primary)
        .status()
        .unwrap()
        .success());
    let linked_cfg = linked_wt.join(".ironlint.yml");
    let store = tempdir().unwrap();
    let store_path = store.path().join("trust.json");
    (cfg, linked_cfg, store_path, script)
}

#[test]
fn blessed_primary_makes_unchanged_sibling_trusted() {
    if !git_available() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let (primary_cfg, linked_cfg, store_path, _script) = worktree_pair();
    bless_in(&primary_cfg, &store_path, "t").unwrap();
    // The sibling has a different canonical path -> direct miss -> worktree hit.
    assert!(
        matches!(
            check_trust_in(&linked_cfg, &store_path),
            TrustOutcome::Trusted
        ),
        "unchanged sibling of a blessed primary must be trusted via inheritance"
    );
}

#[test]
fn legacy_v1_entry_trusts_unchanged_sibling_without_rebless() {
    if !git_available() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let (primary_cfg, linked_cfg, store_path, _script) = worktree_pair();
    bless_in(&primary_cfg, &store_path, "t").unwrap();
    // Downgrade to v1 shape: drop worktree_entries, keep only the direct entry.
    let mut s = ironlint_core::trust::read_store(&store_path).unwrap();
    s.worktree_entries.clear();
    ironlint_core::trust::write_store(&store_path, &s).unwrap();
    let bytes_before = std::fs::read(&store_path).unwrap();
    // Sibling is NOT directly blessed (different canonical path), and there's
    // no v2 worktree entry — but the legacy fallback proves the policy matches.
    assert!(
        matches!(
            check_trust_in(&linked_cfg, &store_path),
            TrustOutcome::Trusted
        ),
        "legacy fallback trusts an unchanged sibling with only a v1 direct entry"
    );
    let bytes_after = std::fs::read(&store_path).unwrap();
    assert_eq!(
        bytes_before, bytes_after,
        "legacy fallback must not mutate the store"
    );
}

#[test]
fn editing_sibling_config_revokes_trust() {
    if !git_available() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let (primary_cfg, linked_cfg, store_path, _script) = worktree_pair();
    bless_in(&primary_cfg, &store_path, "t").unwrap();
    std::fs::write(
        &linked_cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
    )
    .unwrap();
    assert!(
        matches!(
            check_trust_in(&linked_cfg, &store_path),
            TrustOutcome::Untrusted(_)
        ),
        "a changed sibling config must not inherit trust"
    );
}

#[test]
fn editing_sibling_script_revokes_trust() {
    if !git_available() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let (primary_cfg, linked_cfg, store_path, _script) = worktree_pair();
    bless_in(&primary_cfg, &store_path, "t").unwrap();
    // The sibling's script lives under the linked worktree's own
    // .ironlint/scripts/ (git worktree adds a working tree copy).
    let sibling_script = linked_cfg.parent().unwrap().join(".ironlint/scripts/g.sh");
    std::fs::write(&sibling_script, "#!/bin/sh\nexit 0\n").unwrap();
    assert!(
        matches!(
            check_trust_in(&linked_cfg, &store_path),
            TrustOutcome::Untrusted(_)
        ),
        "a changed sibling script must not inherit trust"
    );
}

#[test]
fn separate_clone_with_identical_policy_does_not_inherit() {
    if !git_available() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let (primary_cfg, _linked_cfg, store_path, _script) = worktree_pair();
    bless_in(&primary_cfg, &store_path, "t").unwrap();
    // A completely separate git repo with byte-identical policy -> different
    // common dir -> no inheritance.
    let other = tempdir().unwrap();
    Command::new("git")
        .args(["init", "-q"])
        .arg(other.path())
        .status()
        .unwrap();
    let other_cfg = other.path().join(".ironlint.yml");
    std::fs::write(
        &other_cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \".ironlint/scripts/g.sh\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(other.path().join(".ironlint/scripts")).unwrap();
    std::fs::write(
        other.path().join(".ironlint/scripts/g.sh"),
        "#!/bin/sh\nexit 1\n",
    )
    .unwrap();
    assert!(
        matches!(
            check_trust_in(&other_cfg, &store_path),
            TrustOutcome::Untrusted(_)
        ),
        "a separate clone must not inherit trust"
    );
}
