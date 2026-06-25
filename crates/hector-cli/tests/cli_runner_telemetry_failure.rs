// Telemetry append failures must surface to stderr rather than being dropped.
// Forces a failure by placing a regular file where the runner wants to
// `create_dir_all` a directory, then asserts:
//
// 1. The check still succeeds — telemetry is best-effort, never the
//    source of truth for verdicts.
// 2. stderr contains the "telemetry append failed" diagnostic.

mod common;

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn telemetry_failure_warns_to_stderr() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "clean\n").unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(
        &cfg,
        "gates:\n  noop:\n    files: [\"*.txt\"]\n    run: \"true\"\n",
    )
    .unwrap();

    let xdg = common::blessed_store(&cfg);

    // Pre-place a regular file at `<cfg_dir>/.hector` so that
    // `create_dir_all(".hector")` inside telemetry::append fails (parent
    // exists but is not a directory).
    fs::write(dir.path().join(".hector"), "not a directory").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    // The check itself must still succeed: telemetry is best-effort.
    assert!(
        out.status.success(),
        "check should succeed even when telemetry fails; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("telemetry append failed"),
        "stderr must warn about telemetry failure; got: {stderr}"
    );
}
