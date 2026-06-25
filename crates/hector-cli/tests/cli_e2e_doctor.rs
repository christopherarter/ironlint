//! CLI integration tests for `hector doctor` (gates model).
//!
//! Each test isolates `~/.claude/settings.json` lookup by setting the
//! `HOME` env var to a tempdir, so the adapter check observes a clean
//! environment.

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

fn write_gates_config(dir: &std::path::Path) {
    fs::write(
        dir.join(".hector.yml"),
        "gates:\n  g:\n    files: [\"**/*.rs\"]\n    run: \"true\"\n",
    )
    .unwrap();
}

#[test]
fn doctor_runs_and_reports_binary_check() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_gates_config(dir.path());
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
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
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
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    // All five checks must appear in output.
    for needle in ["binary", "adapter", "config", "parses", "gate_scripts"] {
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
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("parses") && s.contains("fail"),
        "expected `parses fail` for legacy schema: {s}"
    );
}

#[test]
fn doctor_adapter_warn_when_settings_missing() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap(); // empty: no ~/.claude/settings.json
    write_gates_config(dir.path());
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("adapter") && s.contains("warn"),
        "expected `adapter warn`: {s}"
    );
}

#[test]
fn doctor_adapter_pass_when_hook_wired() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    let claude = home.path().join(".claude");
    fs::create_dir_all(&claude).unwrap();
    // Wire a PostToolUse hook whose command references `hector` so the
    // detector recognizes it.
    let settings = r#"{"hooks":{"PostToolUse":[{"matcher":"Edit|Write","hooks":[{"type":"command","command":"hector check --file $HECTOR_FILE"}]}]}}"#;
    fs::write(claude.join("settings.json"), settings).unwrap();
    write_gates_config(dir.path());
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("adapter") && s.contains("ok"),
        "expected `adapter ok`: {s}"
    );
}

#[test]
fn doctor_adapter_warn_when_settings_present_but_no_hector_hook() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    let claude = home.path().join(".claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(
        claude.join("settings.json"),
        r#"{"hooks":{"PostToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"echo unrelated"}]}]}}"#,
    )
    .unwrap();
    write_gates_config(dir.path());
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("adapter") && s.contains("warn"),
        "expected `adapter warn` when no hector hook: {s}"
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
    // Expect 5 checks in gates model: binary, adapter, config, parses, gate_scripts.
    assert_eq!(checks.len(), 5, "gates model doctor has 5 checks: {v}");
    // Each check has name, status, detail.
    for c in checks {
        assert!(c.get("name").is_some());
        assert!(c.get("status").is_some());
        assert!(c.get("detail").is_some());
    }
}
