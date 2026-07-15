//! E2E for `ironlint watch`. In the test harness stdout is piped (not a TTY),
//! so `watch` hits the no-TTY branch: exit 1 with a guidance message. This
//! also exercises `run()`'s entry path for coverage.
use assert_cmd::Command;
use predicates::str::contains;
#[cfg(unix)]
use std::fs;

#[test]
fn watch_without_tty_exits_one_with_message() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("ironlint")
        .unwrap()
        .arg("watch")
        .arg("--dir")
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stderr(contains("requires an interactive terminal"));
}

#[cfg(unix)]
fn project_with_empty_log() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".ironlint")).unwrap();
    fs::write(dir.path().join(".ironlint/log.jsonl"), "").unwrap();
    fs::write(
        dir.path().join(".ironlint.yml"),
        "checks:\n  lint:\n    files: \"**/*.rs\"\n    run: \"true\"\n",
    )
    .unwrap();
    dir
}

#[cfg(unix)]
#[test]
fn watch_uses_a_real_tty_renders_then_quits() {
    let project = project_with_empty_log();
    let mut command = std::process::Command::new(assert_cmd::cargo::cargo_bin!("ironlint"));
    command
        .current_dir(project.path())
        .args(["watch", "--dir", project.path().to_str().unwrap()]);

    let mut session = expectrl::Session::spawn(command).unwrap();
    session.set_expect_timeout(Some(std::time::Duration::from_secs(3)));
    session.expect("waiting for edits").unwrap();
    session.send("q").unwrap();
    session.expect(expectrl::Eof).unwrap();
    assert_eq!(
        session.get_process().wait().unwrap(),
        expectrl::WaitStatus::Exited(session.get_process().pid(), 0)
    );
}
