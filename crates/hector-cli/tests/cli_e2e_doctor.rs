//! CLI integration tests for `hector doctor`.
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
fn doctor_fails_when_trust_fingerprint_broken() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    // Write a config with a *wrong* trust fingerprint.
    let cfg = dir.path().join(".hector.yml");
    let body = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\ntrust:\n  fingerprint: sha256:0000000000000000000000000000000000000000000000000000000000000000\n";
    fs::write(&cfg, body).unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("trust") && s.contains("fail"),
        "expected a failing `trust` row: {s}"
    );
    // Parses-OK before trust-FAIL: distinguish parse failures from trust failures.
    assert!(
        s.contains("parses"),
        "parses check must still appear before trust: {s}"
    );
    assert!(
        s.contains("hector trust"),
        "remediation must hint at `hector trust`: {s}"
    );
}

#[test]
fn doctor_warns_on_legacy_schema_version_one() {
    // schema v1 fails at the parses step (extends::resolve_trusted refuses v1
    // before trust is verified — see config/extends.rs `peek_schema_version`).
    // Doctor must surface that as a `parses` fail with a `hector migrate` hint.
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(&cfg, "schema_version: 1\nrules: {}\n").unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HOME", home.path())
        .args(["doctor", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("hector migrate"),
        "v1 remediation must hint at migrate: {s}"
    );
}

#[test]
fn doctor_passes_on_clean_v2_config() {
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
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    for needle in ["binary", "config", "parses", "trust", "schema"] {
        assert!(s.contains(needle), "expected `{needle}` row in: {s}");
    }
}

#[test]
fn doctor_pass_engines_when_no_llm_rules() {
    // Pure script config — no llm block, no semantic rules. Engines = pass.
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
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("engines") && s.contains("ok"),
        "engines should pass: {s}"
    );
}

#[test]
fn doctor_adapter_warn_when_settings_missing() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap(); // empty: no ~/.claude/settings.json
    write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    );
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
    // detector recognizes it without needing the real adapter installed.
    let settings = r#"{"hooks":{"PostToolUse":[{"matcher":"Edit|Write","hooks":[{"type":"command","command":"hector check --diff -"}]}]}}"#;
    fs::write(claude.join("settings.json"), settings).unwrap();
    write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    );
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
    ).unwrap();
    write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    );
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
    assert!(
        s.contains("docs/adapters/claude-code.md") || s.contains("install"),
        "expected adapter install hint: {s}"
    );
}

#[test]
fn doctor_runtime_state_pass_when_hector_dir_writable() {
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
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("runtime_state"),
        "expected runtime_state row: {s}"
    );
    assert!(
        s.contains("ok"),
        "runtime_state should pass on a fresh tempdir: {s}"
    );
}

#[test]
fn doctor_json_output_snapshot_for_clean_v2_config() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    );
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
    let mut value: serde_json::Value =
        serde_json::from_slice(&out).expect("doctor --format json must produce valid JSON");
    // Redact volatile fields before snapshotting:
    //   - hector_version: changes every release
    //   - per-check `detail`: contains absolute paths and sizes
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "hector_version".into(),
            serde_json::Value::String("[REDACTED]".into()),
        );
    }
    if let Some(checks) = value.get_mut("checks").and_then(|c| c.as_array_mut()) {
        for c in checks {
            if let Some(o) = c.as_object_mut() {
                o.insert(
                    "detail".into(),
                    serde_json::Value::String("[REDACTED]".into()),
                );
                if o.get("remediation").is_some_and(|r| !r.is_null()) {
                    o.insert(
                        "remediation".into(),
                        serde_json::Value::String("[REDACTED]".into()),
                    );
                }
            }
        }
    }
    insta::assert_json_snapshot!(value);
}
