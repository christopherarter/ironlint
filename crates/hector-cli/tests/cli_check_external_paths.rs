/// Paths outside config_dir must be rejected by default.
/// Pass `--allow-external-paths` to opt in.
use assert_cmd::Command;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let cfg = dir.join(".hector.yml");
    std::fs::write(&cfg, body).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    cfg
}

const RULE_YAML: &str = "schema_version: 2\nrules:\n  noop:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n";

/// A file whose canonical path falls outside the config_dir must be rejected
/// with a non-zero exit and a stderr message mentioning "outside" or
/// "external".
#[test]
fn external_path_rejected_by_default() {
    // Two separate temp dirs: one holds the config, the other holds the file.
    let config_dir = tempdir().unwrap();
    let file_dir = tempdir().unwrap();

    let external_file = file_dir.path().join("target.txt");
    std::fs::write(&external_file, "content\n").unwrap();

    let cfg = write_trusted(config_dir.path(), RULE_YAML);

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            external_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    // Must exit non-zero (2 = Block from __internal violation).
    assert_ne!(
        out.status.code(),
        Some(0),
        "expected non-zero exit for external path"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("outside") || stderr.contains("external"),
        "stderr must mention 'outside' or 'external', got: {stderr}"
    );
}

/// With --allow-external-paths, a file outside config_dir is accepted and the
/// noop script rule exits 0.
#[test]
fn external_path_allowed_with_flag() {
    let config_dir = tempdir().unwrap();
    let file_dir = tempdir().unwrap();

    let external_file = file_dir.path().join("target.txt");
    std::fs::write(&external_file, "content\n").unwrap();

    let cfg = write_trusted(config_dir.path(), RULE_YAML);

    Command::cargo_bin("hector")
        .unwrap()
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
