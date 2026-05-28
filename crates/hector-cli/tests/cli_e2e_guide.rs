//! CLI integration tests for `hector guide <file>`.

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

const THREE_RULE_BODY: &str = "schema_version: 2\nrules:\n  ts-rule:\n    description: \"avoid foo in ts\"\n    engine: script\n    scope: [\"**/*.ts\"]\n    severity: error\n    script: \"true\"\n  rs-rule:\n    description: \"no panic in rust\"\n    engine: script\n    scope: [\"**/*.rs\"]\n    severity: warning\n    script: \"true\"\n  any-md:\n    description: \"docs lint\"\n    engine: script\n    scope: [\"*.md\"]\n    severity: warning\n    script: \"true\"\n";

#[test]
fn guide_lists_in_scope_rules_only_with_severity_and_description() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(dir.path(), THREE_RULE_BODY);
    let file = dir.path().join("docs/intro.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "# hi\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "guide",
            "--config",
            cfg.to_str().unwrap(),
            file.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    // Only `any-md` is in scope — rs-rule and ts-rule must not appear.
    assert!(
        stdout.contains("any-md [warning] docs lint"),
        "expected `any-md [warning] docs lint`, got: {stdout}"
    );
    assert!(
        !stdout.contains("ts-rule"),
        "out-of-scope rule must not appear in guide output: {stdout}"
    );
    assert!(
        !stdout.contains("rs-rule"),
        "out-of-scope rule must not appear in guide output: {stdout}"
    );
}

#[test]
fn guide_skipped_file_emits_banner_and_no_rules() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(dir.path(), THREE_RULE_BODY);
    let lock = dir.path().join("Cargo.lock");
    std::fs::write(&lock, "# generated\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "guide",
            "--config",
            cfg.to_str().unwrap(),
            lock.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);
    assert!(
        stdout.contains("SKIPPED"),
        "expected SKIPPED banner: {stdout}"
    );
    assert!(stdout.contains("Cargo.lock"));
    // No rule lines for skipped files.
    assert!(
        !stdout.contains("[warning]"),
        "skipped files have no guidance rows: {stdout}"
    );
    assert!(!stdout.contains("[error]"));
}

#[test]
fn guide_output_is_sorted_by_rule_id() {
    let dir = tempdir().unwrap();
    let body = "schema_version: 2\nrules:\n  zeta:\n    description: \"z\"\n    engine: script\n    scope: [\"*.md\"]\n    severity: warning\n    script: \"true\"\n  alpha:\n    description: \"a\"\n    engine: script\n    scope: [\"*.md\"]\n    severity: error\n    script: \"true\"\n  middle:\n    description: \"m\"\n    engine: script\n    scope: [\"*.md\"]\n    severity: warning\n    script: \"true\"\n";
    let cfg = write_trusted(dir.path(), body);
    let file = dir.path().join("readme.md");
    std::fs::write(&file, "x\n").unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "guide",
            "--config",
            cfg.to_str().unwrap(),
            file.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);
    let alpha_at = stdout.find("alpha").expect("alpha row present");
    let middle_at = stdout.find("middle").expect("middle row present");
    let zeta_at = stdout.find("zeta").expect("zeta row present");
    assert!(alpha_at < middle_at, "alpha must precede middle: {stdout}");
    assert!(middle_at < zeta_at, "middle must precede zeta: {stdout}");
}

#[test]
fn guide_format_json_shape_is_stable() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(dir.path(), THREE_RULE_BODY);
    let file = dir.path().join("docs/intro.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "# hi\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "guide",
            "--config",
            cfg.to_str().unwrap(),
            "--format",
            "json",
            file.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    insta::assert_json_snapshot!("guide_md_file_json", v);
}

#[test]
fn guide_format_json_skipped_file_shape_is_stable() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(dir.path(), THREE_RULE_BODY);
    let lock = dir.path().join("Cargo.lock");
    std::fs::write(&lock, "# generated\n").unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "guide",
            "--config",
            cfg.to_str().unwrap(),
            "--format",
            "json",
            lock.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    insta::assert_json_snapshot!("guide_lockfile_json", v);
}

#[test]
fn guide_missing_config_exits_one() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("foo.md");
    std::fs::write(&file, "x\n").unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "guide",
            "--config",
            dir.path().join(".hector.yml").to_str().unwrap(),
            file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ERROR"));
}
