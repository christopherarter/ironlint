mod common;

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn validate_json_on_good_config_emits_ok() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join(".ironlint.yml");
    fs::write(
        &cfg,
        "checks:\n  py:\n    files: [\"*.py\"]\n    run: \"true\"\n",
    )
    .unwrap();

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["validate", "--format", "json", "--config"])
        .arg(&cfg)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("validate --format json must emit valid JSON; got:\n{stdout}\nerr: {e}")
    });
    assert_eq!(v["status"], "ok");
    assert_eq!(v["checks"], 1);
}

#[test]
fn validate_json_on_bad_config_emits_error_object() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join(".ironlint.yml");
    fs::write(&cfg, "checks: [not, valid, yaml").unwrap();

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["validate", "--format", "json", "--config"])
        .arg(&cfg)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("bad config must still emit a JSON error object on stdout; got:\n{stdout}\nerr: {e}")
    });
    assert_eq!(v["status"], "error");
    assert!(!v["reason"].as_str().unwrap().is_empty());
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn check_json_on_untrusted_config_emits_error_object_not_empty_stdout() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join(".ironlint.yml");
    fs::write(
        &cfg,
        "checks:\n  py:\n    files: [\"*.py\"]\n    run: \"true\"\n",
    )
    .unwrap();
    let src = tmp.path().join("x.py");
    fs::write(&src, "x").unwrap();
    // Deliberately do NOT bless the trust store -> exit 4, untrusted.

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["check", "--file"])
        .arg(&src)
        .arg("--config")
        .arg(&cfg)
        .args(["--format", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("untrusted config in json mode must emit a JSON error object, not empty stdout; got:\n{stdout}\nerr: {e}")
    });
    assert_eq!(v["status"], "error");
    assert_eq!(out.status.code(), Some(4));
}
