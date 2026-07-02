//! CLI integration tests for `ironlint doctor` (gates model).
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
        dir.join(".ironlint.yml"),
        "checks:\n  g:\n    files: [\"**/*.rs\"]\n    run: \"true\"\n",
    )
    .unwrap();
}

/// Run `ironlint doctor --dir <dir>` with a hermetic adapter environment
/// (`HOME` and `XDG_CONFIG_HOME` both under `home`).
fn run_doctor(dir: &Path, home: &Path) -> Output {
    Command::cargo_bin("ironlint")
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
        "doctor output must include the running ironlint version: {s}"
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
        s.contains("ironlint init"),
        "remediation must hint at `ironlint init`: {s}"
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
    for needle in ["binary", "config", "parses", "check_scripts"] {
        assert!(s.contains(needle), "expected `{needle}` row in: {s}");
    }
}

#[test]
fn doctor_parses_fail_on_legacy_schema_config() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    fs::write(
        dir.path().join(".ironlint.yml"),
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
    Command::cargo_bin("ironlint")
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

/// On a repo with a valid config but no trust entry (config never blessed),
/// `doctor` must surface a `trust` row whose status is not `ok` and whose
/// remediation names `ironlint trust`. A read-only command, doctor must NOT
/// exit 1 merely because the config is untrusted — it warns.
#[test]
fn doctor_warns_on_unblessed_config() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_gates_config(dir.path());
    let out = run_doctor(dir.path(), home.path());
    // Read-only: trust is a warn, not a fail → exit 0.
    assert_eq!(
        out.status.code(),
        Some(0),
        "unblessed config must warn, not fail (read-only command): {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("trust"), "doctor must emit a `trust` row: {s}");
    // The trust row must not be `ok` on an unblessed config.
    assert!(
        s.contains("warn") && s.contains("trust"),
        "trust row must warn on an unblessed config: {s}"
    );
    assert!(
        s.contains("ironlint trust"),
        "trust remediation must name `ironlint trust`: {s}"
    );
}

/// On a repo with a valid config but no coding-agent hook wired (no harness
/// detected, none installed), `doctor` must warn that no hooks are detected
/// and remediate with `ironlint init`.
#[test]
fn doctor_warns_when_no_hooks_wired() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap(); // clean machine: nothing detected/installed
    write_gates_config(dir.path());
    let out = run_doctor(dir.path(), home.path());
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("no coding-agent hooks detected"),
        "doctor must warn when no hooks are wired: {s}"
    );
    assert!(
        s.contains("ironlint init"),
        "no-hooks remediation must name `ironlint init`: {s}"
    );
}

/// `doctor --format json` must include the two new rows (trust, hooks) in the
/// `checks` array on a clean, unblessed machine — matching the existing row
/// schema (name/status/detail/remediation).
#[test]
fn doctor_json_includes_trust_and_hooks_rows() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_gates_config(dir.path());
    let out = Command::cargo_bin("ironlint")
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
    let checks = v["checks"].as_array().expect("checks array");
    let names: Vec<&str> = checks.iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert!(
        names.contains(&"trust"),
        "JSON must include a `trust` row: {v}"
    );
    assert!(
        names.contains(&"hooks"),
        "JSON must include a `hooks` row: {v}"
    );
    let trust = checks
        .iter()
        .find(|c| c["name"] == "trust")
        .expect("trust row present");
    assert_ne!(
        trust["status"].as_str().unwrap(),
        "pass",
        "unblessed trust row must not be pass: {v}"
    );
    assert!(
        trust["remediation"]
            .as_str()
            .unwrap()
            .contains("ironlint trust"),
        "trust remediation must name `ironlint trust`: {v}"
    );
    let hooks = checks
        .iter()
        .find(|c| c["name"] == "hooks")
        .expect("hooks row present");
    assert_eq!(
        hooks["status"].as_str().unwrap(),
        "warn",
        "no-hooks machine must warn: {v}"
    );
    assert!(
        hooks["detail"]
            .as_str()
            .unwrap()
            .contains("no coding-agent hooks detected"),
        "hooks detail must explain no hooks detected: {v}"
    );
}

#[test]
fn doctor_json_output_is_valid_for_clean_gates_config() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    write_gates_config(dir.path());
    let out = Command::cargo_bin("ironlint")
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
        v.get("ironlint_version").is_some(),
        "must have ironlint_version"
    );
    assert!(v.get("checks").is_some(), "must have checks array");
    let checks = v["checks"].as_array().unwrap();
    // Clean machine → the seven always-present core rows (no adapter rows,
    // since nothing is detected/installed): binary, config, parses,
    // check_scripts, shell, trust (warn: unblessed), hooks (warn: none wired).
    assert_eq!(checks.len(), 7, "gates model doctor has 7 core checks: {v}");
    // Each check has name, status, detail.
    for c in checks {
        assert!(c.get("name").is_some());
        assert!(c.get("status").is_some());
        assert!(c.get("detail").is_some());
    }
}
