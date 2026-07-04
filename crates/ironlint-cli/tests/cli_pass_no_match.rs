mod common;

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn pass_notes_when_no_checks_matched() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join(".ironlint.yml");
    fs::write(
        &cfg,
        "checks:\n  py:\n    files: [\"*.py\"]\n    run: \"true\"\n",
    )
    .unwrap();
    let src = tmp.path().join("x.txt");
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
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("pass (no checks matched"),
        "expected a no-match note; saw stdout:\n{stdout}"
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "no-match without --require-match stays exit 0"
    );
}

#[test]
fn pass_is_bare_when_checks_matched_and_passed() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join(".ironlint.yml");
    fs::write(
        &cfg,
        "checks:\n  py:\n    files: [\"*.py\"]\n    run: \"true\"\n",
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
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("no checks matched"),
        "a real pass must NOT carry the no-match note; saw stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("pass"),
        "expected a pass line; saw:\n{stdout}"
    );
}

#[test]
fn require_match_makes_no_match_nonzero() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join(".ironlint.yml");
    fs::write(
        &cfg,
        "checks:\n  py:\n    files: [\"*.py\"]\n    run: \"true\"\n",
    )
    .unwrap();
    let src = tmp.path().join("x.txt");
    fs::write(&src, "x").unwrap();
    let xdg = common::blessed_store(&cfg);

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["check", "--file"])
        .arg(&src)
        .arg("--config")
        .arg(&cfg)
        .arg("--require-match")
        .env("XDG_CONFIG_HOME", xdg.path())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "--require-match on a no-match file must exit 2 (Block); saw {:?}",
        out.status
    );
}
