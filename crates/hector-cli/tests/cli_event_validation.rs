//! `--event` is restricted to the four ABI values at the clap layer.
//!
//! Regression coverage for the finding that `--event` was an unvalidated
//! `String`: a typo like `--event percommit` propagated verbatim into
//! `$HECTOR_EVENT`. The ABI enumerates exactly: edit, write, pre-commit, manual.

mod common;

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

const PASSING_CONFIG: &str = "gates:\n  g:\n    files: [\"*.rs\"]\n    run: \"true\"\n";

#[test]
fn event_bogus_is_rejected_and_lists_valid_values() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(&cfg, PASSING_CONFIG).unwrap();
    let file = dir.path().join("lib.rs");
    fs::write(&file, "fn main() {}\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--event",
            "bogus",
        ])
        .output()
        .unwrap();

    assert_ne!(
        out.status.code(),
        Some(0),
        "an invalid --event must not exit 0"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    for v in ["edit", "write", "pre-commit", "manual"] {
        assert!(
            stderr.contains(v),
            "rejection must enumerate the valid event `{v}`: {stderr}"
        );
    }
}

#[test]
fn event_edit_is_accepted() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(&cfg, PASSING_CONFIG).unwrap();
    let file = dir.path().join("lib.rs");
    fs::write(&file, "fn main() {}\n").unwrap();

    let xdg = common::blessed_store(&cfg);

    // `--event edit` parses cleanly and the passing gate yields exit 0.
    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--event",
            "edit",
        ])
        .assert()
        .code(0);
}
