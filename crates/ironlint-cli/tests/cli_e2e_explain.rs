//! CLI integration tests for `ironlint explain <file>` (gates model).
//!
//! Output format (from explain.rs):
//!   <gate-id>  <match|skip>  files=<comma-joined globs>  run=<run>

use assert_cmd::Command;
use tempfile::tempdir;

const TWO_GATE_BODY: &str =
    "checks:\n  ts-gate:\n    files: [\"**/*.ts\"]\n    run: \"true\"\n  rs-gate:\n    files: [\"**/*.rs\"]\n    run: \"true\"\n";

#[test]
fn explain_shows_match_for_file_matching_gate() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    std::fs::write(&cfg, TWO_GATE_BODY).unwrap();

    let file = dir.path().join("lib.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "explain",
            "--config",
            cfg.to_str().unwrap(),
            file.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    // rs-gate should match a .rs file.
    assert!(
        stdout.contains("rs-gate") && stdout.contains("match"),
        "expected `rs-gate match` in stdout, got: {stdout}"
    );
    // ts-gate should skip a .rs file.
    assert!(
        stdout.contains("ts-gate") && stdout.contains("skip"),
        "expected `ts-gate skip` in stdout, got: {stdout}"
    );
}

#[test]
fn explain_line_format_contains_files_and_run() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    std::fs::write(&cfg, TWO_GATE_BODY).unwrap();

    let file = dir.path().join("lib.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "explain",
            "--config",
            cfg.to_str().unwrap(),
            file.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    // Each line must have files= and run= per explain.rs format.
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        assert!(
            line.contains("files="),
            "explain line must contain `files=`: {line}"
        );
        assert!(
            line.contains("run="),
            "explain line must contain `run=`: {line}"
        );
    }
}

#[test]
fn explain_missing_config_exits_one() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "x\n").unwrap();
    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "explain",
            "--config",
            dir.path().join(".ironlint.yml").to_str().unwrap(),
            file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error:"),
        "missing config must surface a stderr error: {stderr}"
    );
}
