//! Covers the production `resolve_timeout` log path in `ironlint-core`: when an
//! ambient `IRONLINT_TIMEOUT` override parses below the 10s floor, the runner
//! raises it to the floor AND emits a loud `eprintln!` explaining the raise.
//! The pure wrapper `resolve_timeout_with_floor` is unit-tested directly in
//! `runner.rs`; this test exercises the caller's `if shortened { eprintln! }`
//! branch end-to-end via the compiled `ironlint` binary's stderr. Stable Rust
//! cannot capture `eprintln!` in-process, so we mirror the `assert_cmd`
//! subprocess pattern proven by `cli_check_single_load.rs`.
mod common;

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn cli_check_warns_when_ambient_timeout_below_floor() {
    let tmp = tempdir().unwrap();
    let cfg_path = tmp.path().join(".ironlint.yml");
    fs::write(
        &cfg_path,
        "checks:\n  noop:\n    files: [\"*\"]\n    run: \"true\"\n",
    )
    .unwrap();
    let src = tmp.path().join("x.txt");
    fs::write(&src, "x").unwrap();

    // Bless the config so the CLI `check` trust gate (exit 4) does not fire
    // before the engine loads and `resolve_timeout` runs.
    let xdg = common::blessed_store(&cfg_path);

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["check", "--file"])
        .arg(&src)
        .arg("--config")
        .arg(&cfg_path)
        .env("XDG_CONFIG_HOME", xdg.path())
        .env("IRONLINT_TIMEOUT", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The check passes (`run: "true"`), but the warning must still appear —
    // the floor log fires on load, independent of the per-check verdict.
    assert!(
        stderr.contains("ironlint: IRONLINT_TIMEOUT=1 is below the 10s floor"),
        "expected the timeout-floor warning on stderr; saw:\n{stderr}"
    );
}

#[test]
fn cli_check_silent_when_ambient_timeout_meets_floor() {
    let tmp = tempdir().unwrap();
    let cfg_path = tmp.path().join(".ironlint.yml");
    fs::write(
        &cfg_path,
        "checks:\n  noop:\n    files: [\"*\"]\n    run: \"true\"\n",
    )
    .unwrap();
    let src = tmp.path().join("x.txt");
    fs::write(&src, "x").unwrap();

    let xdg = common::blessed_store(&cfg_path);

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["check", "--file"])
        .arg(&src)
        .arg("--config")
        .arg(&cfg_path)
        .env("XDG_CONFIG_HOME", xdg.path())
        // Exactly at the floor — not "below", so no warning.
        .env("IRONLINT_TIMEOUT", "10")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("is below the 10s floor"),
        "expected NO timeout-floor warning at the floor; saw:\n{stderr}"
    );
}
