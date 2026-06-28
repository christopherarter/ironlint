//! CLI integration tests for `hector doctor` (gates model).
//!
//! Each test isolates the adapter environment by pointing `HOME` and
//! `XDG_CONFIG_HOME` at tempdirs, so the per-harness adapter checks observe a
//! clean machine (no harness detected, nothing installed) unless the test
//! installs one itself.

use assert_cmd::Command;
use std::fs;
use std::path::Path;
use std::process::Output;
use tempfile::tempdir;

fn write_gates_config(dir: &Path) {
    fs::write(
        dir.join(".hector.yml"),
        "checks:\n  g:\n    files: [\"**/*.rs\"]\n    run: \"true\"\n",
    )
    .unwrap();
}

/// Run `hector doctor --dir <dir>` with a hermetic adapter environment
/// (`HOME` and `XDG_CONFIG_HOME` both under `home`).
fn run_doctor(dir: &Path, home: &Path) -> Output {
    Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .args(["doctor", "--dir", dir.to_str().unwrap()])
        .output()
        .unwrap()
}

#[test]
fn doctor_runs_and_reports_binary_check() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_gates_config(dir.path());
    let out = run_doctor(dir.path(), home.path());
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("binary"),
        "doctor output must mention the binary check: {s}"
    );
    assert!(
        s.contains(env!("CARGO_PKG_VERSION")),
        "doctor output must include the running hector version: {s}"
    );
}

#[test]
fn doctor_fails_when_config_missing() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    let out = run_doctor(dir.path(), home.path());
    assert_eq!(out.status.code(), Some(1), "missing config must exit 1");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("config") && s.contains("fail"),
        "expected a failing `config` row: {s}"
    );
    assert!(
        s.contains("hector init"),
        "remediation must hint at `hector init`: {s}"
    );
}

#[test]
fn doctor_passes_on_clean_gates_config() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_gates_config(dir.path());
    let out = run_doctor(dir.path(), home.path());
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    // The four always-present gate-model checks must appear.
    for needle in ["binary", "config", "parses", "gate_scripts"] {
        assert!(s.contains(needle), "expected `{needle}` row in: {s}");
    }
}

#[test]
fn doctor_parses_fail_on_legacy_schema_config() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    fs::write(
        dir.path().join(".hector.yml"),
        "schema_version: 2\nrules: {}\n",
    )
    .unwrap();
    let out = run_doctor(dir.path(), home.path());
    assert_eq!(out.status.code(), Some(1));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("parses") && s.contains("fail"),
        "expected `parses fail` for legacy schema: {s}"
    );
}

#[test]
fn doctor_omits_adapter_rows_on_clean_machine() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap(); // no harness installed or detected
    write_gates_config(dir.path());
    let out = run_doctor(dir.path(), home.path());
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    // No harness is present, so no per-harness adapter row is emitted.
    for harness in ["claude-code", "reasonix", "pi", "opencode"] {
        assert!(
            !s.contains(harness),
            "clean machine must not emit a `{harness}` adapter row: {s}"
        );
    }
}

#[test]
fn doctor_reports_installed_reasonix_adapter() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_gates_config(dir.path());
    // Wire the reasonix hook via the real install path, then re-run doctor.
    Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args([
            "init",
            "--dir",
            dir.path().to_str().unwrap(),
            "--harness",
            "reasonix",
            "--global",
            "--hook-only",
            "--yes",
        ])
        .assert()
        .success();
    let out = run_doctor(dir.path(), home.path());
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("reasonix") && s.contains("ok"),
        "expected a passing `reasonix` adapter row: {s}"
    );
}

#[test]
fn doctor_json_output_is_valid_for_clean_gates_config() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_gates_config(dir.path());
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args([
            "doctor",
            "--dir",
            dir.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value =
        serde_json::from_slice(&out).expect("doctor --format json must produce valid JSON");
    // Top-level fields present.
    assert!(
        v.get("hector_version").is_some(),
        "must have hector_version"
    );
    assert!(v.get("checks").is_some(), "must have checks array");
    let checks = v["checks"].as_array().unwrap();
    // Clean machine → only the four core gate-model checks (no adapter rows):
    // binary, config, parses, gate_scripts.
    assert_eq!(checks.len(), 4, "gates model doctor has 4 core checks: {v}");
    // Each check has name, status, detail.
    for c in checks {
        assert!(c.get("name").is_some());
        assert!(c.get("status").is_some());
        assert!(c.get("detail").is_some());
    }
}
