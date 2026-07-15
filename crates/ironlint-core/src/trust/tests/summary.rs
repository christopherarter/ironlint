use super::*;
use std::fs;
use std::path::Path;
use std::process::Command;

fn write(p: &Path, body: &str) {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, body).unwrap();
}

/// Build a real git repo at `root` so `WorktreeScope::discover` succeeds.
fn git_repo(root: &Path) {
    fs::create_dir_all(root).unwrap();
    let _ = Command::new("git").args(["init", "-q"]).arg(root).status();
    // .ironlint.yml is the trust surface; commit is unnecessary for discovery.
}

#[test]
fn blessed_summary_lists_hash_checks_and_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    write(
        &cfg,
        "checks:\n  g:\n    files: \"*\"\n    run: \"bash scripts/lint.sh\"\n",
    );
    write(&dir.path().join(".ironlint/scripts/a.sh"), "a\n");
    write(&dir.path().join(".ironlint/scripts/b.sh"), "b\n");
    write(&dir.path().join("scripts/lint.sh"), "#!/bin/sh\nexit 0\n");

    let summary = blessed_summary(&cfg).unwrap();

    assert!(
        summary.config_hash.starts_with("sha256:"),
        "config_hash must be sha256-prefixed: {}",
        summary.config_hash
    );
    assert_eq!(
        summary.config_hash,
        compute_hash(&cfg).unwrap(),
        "blessed_summary must report the SAME digest compute_hash would produce"
    );
    assert_eq!(summary.checks, 1, "checks counts resolved checks");
    assert_eq!(
        summary.scripts,
        vec!["a.sh".to_string(), "b.sh".to_string()],
        "scripts lists every file under .ironlint/scripts/, sorted"
    );
}

#[test]
fn blessed_summary_is_empty_with_no_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
    );

    let summary = blessed_summary(&cfg).unwrap();

    assert!(
        summary.scripts.is_empty(),
        "no scripts dir → empty scripts list"
    );
    assert_eq!(summary.checks, 1, "the one inline check still counts");
}

#[test]
fn blessed_summary_scope_is_linked_worktrees_for_git_repo() {
    let root = tempfile::tempdir().unwrap();
    git_repo(root.path());
    let cfg = root.path().join(".ironlint.yml");
    write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
    );
    let sum = blessed_summary(&cfg).unwrap();
    assert_eq!(sum.scope, "linked worktrees");
}

#[test]
fn blessed_summary_scope_is_this_config_path_when_not_git() {
    let root = tempfile::tempdir().unwrap();
    let cfg = root.path().join(".ironlint.yml");
    write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
    );
    let sum = blessed_summary(&cfg).unwrap();
    assert_eq!(sum.scope, "this config path");
}
