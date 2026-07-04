mod common;

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn internal_error_renders_detail_with_run_and_timeout() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join(".ironlint.yml");
    // A check whose `run` references a command that won't be found -> not_found
    // internal error. Short timeout to exercise the timeout path is racy; this
    // test asserts the not_found detail shape, which is deterministic.
    fs::write(
        &cfg,
        "checks:\n  boom:\n    files: [\"*.py\"]\n    run: \"definitely-not-a-real-cmd-xyz\"\n",
    )
    .unwrap();
    let src = tmp.path().join("x.py");
    fs::write(&src, "x").unwrap();
    let xdg = common::blessed_store(&cfg);

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["check", "--file"])
        .arg(&src)
        .arg("--config")
        .arg(&cfg)
        .env("XDG_CONFIG_HOME", xdg.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("running: definitely-not-a-real-cmd-xyz"),
        "internal-error detail must name the run command; saw stderr:\n{stderr}"
    );
    assert_eq!(out.status.code(), Some(3), "internal error must exit 3");
}
