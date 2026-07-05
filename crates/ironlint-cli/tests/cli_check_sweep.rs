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

#[test]
fn pre_commit_check_runs_once_over_the_matched_set() {
    let project = project_with_config(concat!(
        "checks:\n",
        "  set-check:\n",
        "    files: \"*.md\"\n",
        "    on: [pre-commit]\n",
        "    run: |\n",
        "      echo INVOKED >> invocations.txt\n",
        "      printf '%s\\n' \"$IRONLINT_FILES\" >> seen.txt\n",
        "      exit 0\n",
    ));
    fs::write(project.path().join("a.md"), "x\n").unwrap();
    fs::write(project.path().join("b.md"), "y\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    ironlint(&project, &xdg).arg("check").assert().code(0);

    let invocations = fs::read_to_string(project.path().join("invocations.txt")).unwrap();
    assert_eq!(
        invocations.lines().count(),
        1,
        "expected exactly one batched invocation, got: {invocations}"
    );
    let seen = fs::read_to_string(project.path().join("seen.txt")).unwrap();
    assert!(seen.contains("a.md"), "seen: {seen}");
    assert!(seen.contains("b.md"), "seen: {seen}");
    assert_eq!(
        seen.lines().count(),
        2,
        "expected both files in one $IRONLINT_FILES, got: {seen}"
    );
}

#[test]
fn dual_lifecycle_check_is_batched_not_double_run() {
    let project = project_with_config(concat!(
        "checks:\n",
        "  dual:\n",
        "    files: \"*.md\"\n",
        "    on: [write, pre-commit]\n",
        "    run: 'echo INVOKED >> invocations.txt; exit 0'\n",
    ));
    fs::write(project.path().join("a.md"), "x\n").unwrap();
    fs::write(project.path().join("b.md"), "y\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    ironlint(&project, &xdg).arg("check").assert().code(0);

    let invocations = fs::read_to_string(project.path().join("invocations.txt")).unwrap();
    assert_eq!(
        invocations.lines().count(),
        1,
        "a dual-lifecycle check must run exactly once per sweep, got: {invocations}"
    );
}

#[test]
fn batched_check_block_reaches_the_sweep_verdict() {
    let project = project_with_config(
        "checks:\n  always-block:\n    files: \"*.md\"\n    on: [pre-commit]\n    run: 'echo set-level violation; exit 1'\n",
    );
    fs::write(project.path().join("a.md"), "x\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    let assert = ironlint(&project, &xdg).arg("check").assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("always-block"), "stderr: {stderr}");
    assert!(stderr.contains("set-level violation"), "stderr: {stderr}");
}

#[test]
fn sweep_honors_gitignore_without_a_git_repo() {
    let project = project_with_config(GREP_CHECK);
    fs::write(project.path().join(".gitignore"), "vendor/\n").unwrap();
    fs::create_dir_all(project.path().join("vendor")).unwrap();
    fs::write(
        project.path().join("vendor/dirty.md"),
        "has FORBIDDEN word\n",
    )
    .unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    ironlint(&project, &xdg).arg("check").assert().code(0);
}

#[test]
fn sweep_skips_hidden_directories() {
    let project = project_with_config(GREP_CHECK);
    fs::create_dir_all(project.path().join(".secrets")).unwrap();
    fs::write(
        project.path().join(".secrets/dirty.md"),
        "has FORBIDDEN word\n",
    )
    .unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    ironlint(&project, &xdg).arg("check").assert().code(0);
}

#[test]
fn sweep_warns_and_skips_non_utf8_but_still_blocks_siblings() {
    let project = project_with_config(GREP_CHECK);
    fs::write(project.path().join("binary.md"), [0xFF, 0xFE, 0x00, 0x01]).unwrap();
    fs::write(project.path().join("dirty.md"), "has FORBIDDEN word\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    let assert = ironlint(&project, &xdg).arg("check").assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("WARNING") && stderr.contains("binary.md"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("dirty.md"), "stderr: {stderr}");
}

#[test]
fn sweep_check_filter_limits_to_named_check() {
    let project = project_with_config(concat!(
        "checks:\n",
        "  no-forbidden:\n",
        "    files: \"*.md\"\n",
        "    run: '! grep -n FORBIDDEN'\n",
        "  always-block:\n",
        "    files: \"*.txt\"\n",
        "    run: 'exit 1'\n",
    ));
    fs::write(project.path().join("clean.md"), "all good\n").unwrap();
    fs::write(project.path().join("note.txt"), "anything\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    // Only no-forbidden runs; always-block's guaranteed violation is filtered out.
    ironlint(&project, &xdg)
        .args(["check", "--check", "no-forbidden"])
        .assert()
        .code(0);
}

#[test]
fn sweep_require_match_flags_a_scope_that_matches_nothing() {
    let project =
        project_with_config("checks:\n  ghost:\n    files: \"*.nomatch\"\n    run: 'exit 1'\n");
    fs::write(project.path().join("a.md"), "x\n").unwrap();
    let xdg = blessed_store(&project.path().join(".ironlint.yml"));

    ironlint(&project, &xdg).arg("check").assert().code(0);
    ironlint(&project, &xdg)
        .args(["check", "--require-match"])
        .assert()
        .code(2);
}

#[test]
fn sweep_fails_closed_on_untrusted_config() {
    let project = project_with_config(GREP_CHECK);
    fs::write(project.path().join("a.md"), "x\n").unwrap();
    // Fresh, empty trust store: the config was never blessed.
    let empty_xdg = tempfile::tempdir().unwrap();

    ironlint(&project, &empty_xdg).arg("check").assert().code(4);
}
