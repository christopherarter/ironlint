use assert_cmd::Command;
use tempfile::tempdir;

#[test]
fn validate_accepts_valid_checks_config() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    std::fs::write(
        &cfg,
        "checks:\n  g:\n    files: [\"**/*.rs\"]\n    run: \"true\"\n",
    )
    .unwrap();
    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(s.contains("ok"), "validate must print ok on success: {s}");
    assert!(
        s.contains("1 check(s)") || s.contains("1 check"),
        "validate must print check count: {s}"
    );
}

#[test]
fn validate_accepts_multi_check_config() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    std::fs::write(
        &cfg,
        "checks:\n  a:\n    files: [\"*.rs\"]\n    run: \"true\"\n  b:\n    files: [\"*.ts\"]\n    run: \"true\"\n",
    )
    .unwrap();
    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(
        s.contains("2 check(s)") || s.contains("2 check"),
        "validate must print check count: {s}"
    );
}

#[test]
fn validate_rejects_legacy_rules_config() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    std::fs::write(&cfg, "schema_version: 2\nrules: {}\n").unwrap();
    Command::cargo_bin("ironlint")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn validate_rejects_unknown_check_field() {
    // `exclude:` was a real field in the pre-0.3 engine model; a 0.4 check is
    // exactly `{ files, run }`. A stale/typo'd field must hard-error at validate
    // time (exit 1) and name the offending field — never be silently dropped.
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    std::fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.ts\"\n    exclude: \"*.test.ts\"\n    run: \"true\"\n",
    )
    .unwrap();
    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .get_output()
        .stderr
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(
        s.contains("exclude"),
        "validate error must name the unknown field: {s}"
    );
}

#[test]
fn validate_rejects_run_with_no_executable_content() {
    // A `run:` that collapses to a single `#` comment (the folded-YAML-scalar
    // footgun) is a check that silently passes everything. validate must reject
    // it with exit 1 rather than bless a no-op check.
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    std::fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*\"\n    run: \"# todo: write this check\"\n",
    )
    .unwrap();
    Command::cargo_bin("ironlint")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn validate_rejects_bad_yaml() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    std::fs::write(&cfg, "not: valid: yaml: :\n").unwrap();
    Command::cargo_bin("ironlint")
        .unwrap()
        .args(["validate", "--config", cfg.to_str().unwrap()])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn validate_rejects_removed_architecture_key() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join(".ironlint.yml");
    std::fs::write(
        &config,
        "architecture:\n  layers:\n    - name: data\n      globs: [\"src/data/**\"]\nchecks: {}\n",
    )
    .unwrap();

    let output = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["validate", "--config", config.to_str().unwrap()])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown field `architecture`"), "{stderr}");
}

#[test]
fn validate_accepts_arch_as_ordinary_check_id() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join(".ironlint.yml");
    std::fs::write(
        &config,
        "checks:\n  __arch__:\n    files: \"**/*\"\n    run: \"true\"\n",
    )
    .unwrap();

    let output = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["validate", "--config", config.to_str().unwrap()])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
}
