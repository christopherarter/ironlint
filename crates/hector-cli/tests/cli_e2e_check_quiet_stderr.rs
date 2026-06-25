//! Routine `hector check` invocations must stay quiet on stderr for
//! passing gates. No diagnostic noise should appear on routine checks.

mod common;

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn check_stays_quiet_on_stderr_for_passing_gate() {
    let dir = tempdir().unwrap();
    let project = dir.path();

    let cfg = project.join(".hector.yml");
    fs::write(
        &cfg,
        "gates:\n  ok:\n    files: [\"*.txt\"]\n    run: \"exit 0\"\n",
    )
    .unwrap();

    let file = project.join("ok.txt");
    fs::write(&file, "fine\n").unwrap();

    let xdg = common::blessed_store(&cfg);

    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(0)
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8_lossy(&out);
    assert!(
        stderr.is_empty(),
        "routine `hector check` against a passing gate must keep stderr empty; got: {stderr:?}"
    );
}
