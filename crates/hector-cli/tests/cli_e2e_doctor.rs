//! C1 — CLI integration tests for `hector doctor`.
//!
//! Each test isolates `~/.claude/settings.json` lookup by setting the
//! `HOME` env var to a tempdir, so the adapter check observes a clean
//! environment. The doctor module honors `HOME` via the `home_dir`
//! helper it inherits from `runner.rs`.

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let cfg = dir.join(".hector.yml");
    fs::write(&cfg, body).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    cfg
}

#[test]
fn doctor_runs_and_reports_binary_check() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(s.contains("binary"), "doctor output must mention the binary check: {s}");
    assert!(s.contains(env!("CARGO_PKG_VERSION")), "doctor output must include the running hector version: {s}");
}
