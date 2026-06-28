/// Paths outside config_dir must be rejected by default.
/// Pass `--allow-external-paths` to opt in.
mod common;

use assert_cmd::Command;
use tempfile::tempdir;

const GATE_YAML: &str = "checks:\n  noop:\n    files: [\"*\"]\n    run: \"true\"\n";

/// A file whose canonical path falls outside the config_dir must be rejected
/// with exit 1 and a stderr message mentioning "outside" or "external".
#[test]
fn external_path_rejected_by_default() {
    // Two separate temp dirs: one holds the config, the other holds the file.
    let config_dir = tempdir().unwrap();
    let file_dir = tempdir().unwrap();

    let external_file = file_dir.path().join("target.txt");
    std::fs::write(&external_file, "content\n").unwrap();

    let cfg = config_dir.path().join(".hector.yml");
    std::fs::write(&cfg, GATE_YAML).unwrap();

    let xdg = common::blessed_store(&cfg);

    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            external_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    // An external-path rejection is an argument/config error: exit 1
    // (the documented bucket), never exit 3 (InternalError) — exit 3 makes
    // adapters fail OPEN, silently defeating this guard.
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 (argument error) for external path, got {:?}",
        out.status.code()
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("outside") || stderr.contains("external"),
        "stderr must mention 'outside' or 'external', got: {stderr}"
    );
}

/// With --allow-external-paths, a file outside config_dir is accepted.
#[test]
fn external_path_allowed_with_flag() {
    let config_dir = tempdir().unwrap();
    let file_dir = tempdir().unwrap();

    let external_file = file_dir.path().join("target.txt");
    std::fs::write(&external_file, "content\n").unwrap();

    let cfg = config_dir.path().join(".hector.yml");
    std::fs::write(&cfg, GATE_YAML).unwrap();

    let xdg = common::blessed_store(&cfg);

    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            external_file.to_str().unwrap(),
            "--allow-external-paths",
        ])
        .assert()
        .code(0);
}
