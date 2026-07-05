//! E2E: bare `ironlint check` sweeps the repo (no --file / --diff).

mod common;

use assert_cmd::Command;
use common::blessed_store;
use std::fs;
use tempfile::TempDir;

fn project_with_config(yaml: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(".ironlint.yml"), yaml).unwrap();
    dir
}

fn ironlint(project: &TempDir, xdg: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.current_dir(project.path())
        .env("XDG_CONFIG_HOME", xdg.path());
    cmd
}

const GREP_CHECK: &str =
    "checks:\n  no-forbidden:\n    files: \"*.md\"\n    run: '! grep -n FORBIDDEN'\n";

#[test]
fn bare_check_sweeps_write_only_checks_across_nested_dirs() {
    let project = project_with_config(GREP_CHECK);
    fs::create_dir_all(project.path().join("docs/deep")).unwrap();
    fs::write(project.path().join("clean.md"), "all good\n").unwrap();
    fs::write(
        project.path().join("docs/deep/dirty.md"),
        "has FORBIDDEN word\n",
    )
    .unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    let assert = ironlint(&project, &xdg).arg("check").assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("no-forbidden"), "stderr: {stderr}");
    assert!(stderr.contains("docs/deep/dirty.md"), "stderr: {stderr}");
    // The clean file must not appear as a block.
    assert!(!stderr.contains("clean.md"), "stderr: {stderr}");
}

#[test]
fn bare_check_on_clean_repo_exits_zero() {
    let project = project_with_config(GREP_CHECK);
    fs::write(project.path().join("clean.md"), "all good\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    let assert = ironlint(&project, &xdg).arg("check").assert().code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(stdout.contains("pass"), "stdout: {stdout}");
}

#[test]
fn bare_check_rejects_explicit_event() {
    let project = project_with_config(GREP_CHECK);
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    let assert = ironlint(&project, &xdg)
        .args(["check", "--event", "write"])
        .assert()
        .code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("--event requires --file or --diff"),
        "stderr: {stderr}"
    );
}

#[test]
fn bare_check_rejects_force() {
    let project = project_with_config(GREP_CHECK);
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    let assert = ironlint(&project, &xdg)
        .args(["check", "--force", "--check", "no-forbidden"])
        .assert()
        .code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("--force requires --file"),
        "stderr: {stderr}"
    );
}
