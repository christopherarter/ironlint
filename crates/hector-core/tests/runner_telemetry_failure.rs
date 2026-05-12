// P2-21: telemetry append failures used to be silently dropped — the
// runner logged `let _ = crate::telemetry::append(...)` at three sites
// in `runner.rs`. If the log directory was unwritable (read-only mount,
// disk full, parent path collision), the user got zero diagnostic.
//
// The runner now wraps each append in `if let Err(e) = … { eprintln! … }`.
// This regression test forces telemetry to fail by pre-placing a regular
// FILE at the path where the runner wants to `create_dir_all` a
// directory, then asserts:
//
// 1. The check still succeeds — telemetry is best-effort, never the
//    source of truth for verdicts.
// 2. stderr contains the "telemetry append failed" diagnostic.

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
        "schema_version: 2\nrules:\n  noop:\n    description: x\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    )
    .unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();

    // Pre-place a regular file at `<cfg_dir>/.hector` so that
    // `create_dir_all(".hector")` inside telemetry::append fails (parent
    // exists but is not a directory). The append at the end of the
    // `check` call will then return an error that the runner must
    // surface to stderr instead of dropping silently.
    fs::write(dir.path().join(".hector"), "not a directory").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
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
