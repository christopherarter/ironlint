//! CLI-level linked-worktree trust inheritance. Proves check EXECUTES (exit 2)
//! in a trusted sibling, and that `ironlint trust` prints the scope line + the
//! existing summary, and writes both entries.

use assert_cmd::Command;
use std::process::Command as StdCommand;
use tempfile::tempdir;

fn git_available() -> bool {
    StdCommand::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn trusted_sibling_runs_a_blocking_check_and_exits_2() {
    if !git_available() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let xdg = tempdir().unwrap();
    // Convert to plain paths so the directories survive the helper's return.
    let primary = tempdir().unwrap().keep();
    let linked = tempdir().unwrap().keep();
    StdCommand::new("git")
        .args(["init", "-q"])
        .arg(&primary)
        .status()
        .unwrap();
    let cfg = primary.join(".ironlint.yml");
    std::fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"sh .ironlint/scripts/g.sh\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(primary.join(".ironlint/scripts")).unwrap();
    std::fs::write(
        primary.join(".ironlint/scripts/g.sh"),
        "#!/bin/sh\necho blocked; exit 1\n",
    )
    .unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(&primary)
        .status()
        .unwrap();
    StdCommand::new("git")
        .args([
            "-c",
            "user.email=t@t.co",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "i",
        ])
        .current_dir(&primary)
        .status()
        .unwrap();
    let linked_wt = linked.join("wt");
    StdCommand::new("git")
        .args(["worktree", "add", "-q"])
        .arg(&linked_wt)
        .current_dir(&primary)
        .status()
        .unwrap();
    // Bless the primary through the CLI.
    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(&cfg)
        .assert()
        .success()
        .stdout(predicates::str::contains("scope: linked worktrees"))
        .stdout(predicates::str::contains("config sha256:"))
        .stdout(predicates::str::contains("checks:"))
        .stdout(predicates::str::contains("scripts:"));
    // In the sibling: check runs the blocking gate -> exit 2.
    let linked_cfg = linked_wt.join(".ironlint.yml");
    std::fs::write(linked_wt.join("dummy.rs"), "fn main() {}\n").unwrap();
    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["check", "--config"])
        .arg(&linked_cfg)
        .arg("--file")
        .arg(linked_wt.join("dummy.rs"))
        .assert()
        .failure()
        .code(2);
    let store = std::fs::read_to_string(xdg.path().join("ironlint/trust.json")).unwrap();
    assert!(
        store.contains("\"entries\""),
        "store has a direct entries field: {store}"
    );
    assert!(
        store.contains("worktree_entries"),
        "store has a worktree_entries field: {store}"
    );
}
