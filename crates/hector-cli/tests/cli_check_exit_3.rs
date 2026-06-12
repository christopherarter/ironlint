//! Contract test: `hector check` exits 3 on an InternalError verdict.
//!
//! A file whose canonical path resolves outside the config directory triggers
//! an `__internal` violation (Engine::Internal), which `Verdict::from_violations`
//! maps to `Status::InternalError`, which the CLI maps to exit code 3.
//!
//! This covers the same exit-code contract that the deleted semantic/missing-
//! API-key test covered — the engine path is different but the contract
//! (`__internal` violation → InternalError → exit 3) is identical.
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
fn cli_check_exit_3_on_internal_error() {
    // Config in one tempdir, target file in another (outside config_dir).
    // Without --allow-external-paths, resolve_input_path returns Err, which
    // produces an Engine::Internal __internal violation → Status::InternalError
    // → exit 3.
    let config_dir = tempdir().unwrap();
    let file_dir = tempdir().unwrap();

    let cfg = write_trusted(
        config_dir.path(),
        "schema_version: 2\nrules:\n  guard:\n    description: \"must not contain forbidden\"\n    engine: script\n    scope: [\"**/*.rs\"]\n    severity: error\n    script: \"exit 1\"\n",
    );

    // A real file that exists, but lives outside config_dir.
    let external = file_dir.path().join("subject.rs");
    fs::write(&external, "fn main() {}\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            external.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(3),
        "external-path rejection must exit 3 (InternalError), not {}; \
         stdout: {}\nstderr: {}",
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // The verdict JSON printed to stdout must contain __internal and
    // internal_error so callers know not to treat this as a policy Block.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("__internal"),
        "stdout must contain '__internal' violation; got: {stdout}"
    );
    assert!(
        stdout.contains("internal_error"),
        "stdout must surface 'internal_error' status; got: {stdout}"
    );
}
