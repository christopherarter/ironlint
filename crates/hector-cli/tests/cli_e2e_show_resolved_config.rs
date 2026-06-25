//! End-to-end coverage for `hector show-resolved-config` (gates model).
//!
//! Output format (from show_resolved_config.rs):
//!   <gate-id>  (from <origin-path>)
//!     files: <comma-joined globs>
//!     run: <run>

use assert_cmd::Command;
use tempfile::tempdir;

#[test]
fn show_resolved_config_prints_gate_id_files_run() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(
        &cfg,
        "gates:\n  no-todo:\n    files: [\"*.rs\", \"*.txt\"]\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
    )
    .unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["show-resolved-config", "--config", cfg.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    assert!(stdout.contains("no-todo"), "must print gate id: {stdout}");
    assert!(
        stdout.contains("files:"),
        "must print files: field: {stdout}"
    );
    assert!(stdout.contains("run:"), "must print run: field: {stdout}");
    assert!(
        stdout.contains("(from"),
        "must print origin path with (from ...): {stdout}"
    );
    assert!(
        stdout.contains(".hector.yml"),
        "origin must reference the config file: {stdout}"
    );
}

#[test]
fn show_resolved_config_lists_multiple_gates() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(
        &cfg,
        "gates:\n  alpha:\n    files: [\"*.rs\"]\n    run: \"true\"\n  beta:\n    files: [\"*.ts\"]\n    run: \"true\"\n",
    )
    .unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["show-resolved-config", "--config", cfg.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    assert!(stdout.contains("alpha"), "must show alpha gate: {stdout}");
    assert!(stdout.contains("beta"), "must show beta gate: {stdout}");
}

#[test]
fn show_resolved_config_missing_config_exits_one() {
    let dir = tempdir().unwrap();
    let absent = dir.path().join(".hector.yml");
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["show-resolved-config", "--config", absent.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("ERROR: "),
        "stderr must lead with ERROR: prefix: {stderr}"
    );
}
