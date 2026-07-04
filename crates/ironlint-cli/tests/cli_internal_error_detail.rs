mod common;

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn internal_error_detail_names_the_run_command() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join(".ironlint.yml");
    // A check whose `run` references a command that won't be found -> not_found
    // internal error. Exercises the `detail_for` non-timeout arm, which renders
    // "<reason> running: <cmd>". The timeout arm is unit-tested in core
    // (`detail_for` formats "timeout after Ns"); a CLI timeout case is racy
    // (timing-dependent), so this test sticks to the deterministic not_found
    // path and asserts the run command appears in the rendered detail.
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
