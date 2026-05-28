//! CLI integration tests for `hector explain <file>`.

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
fn explain_prints_match_and_skip_lines_for_a_markdown_file() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(dir.path(), THREE_RULE_BODY);
    let file = dir.path().join("docs/intro.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "# hi\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "explain",
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

    // The match line uses `MATCH` (uppercase) so it greps distinctly.
    assert!(
        stdout.contains("MATCH any-md via *.md"),
        "expected `MATCH any-md via *.md` in stdout, got: {stdout}"
    );
    // Non-matching rules use lowercase `skip` (also distinct under grep).
    assert!(
        stdout.contains("skip ts-rule scope=**/*.ts"),
        "expected `skip ts-rule scope=**/*.ts` in stdout, got: {stdout}"
    );
    assert!(
        stdout.contains("skip rs-rule scope=**/*.rs"),
        "expected `skip rs-rule scope=**/*.rs` in stdout, got: {stdout}"
    );
}

#[test]
fn explain_emits_skipped_banner_for_lockfile() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(dir.path(), THREE_RULE_BODY);
    let lock = dir.path().join("Cargo.lock");
    std::fs::write(&lock, "# generated\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "explain",
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
        "lockfile must produce a SKIPPED banner: {stdout}"
    );
    assert!(
        stdout.contains("Cargo.lock"),
        "the SKIPPED banner names the matching skip pattern: {stdout}"
    );
    // Per-rule rows still emit so the author sees the full picture.
    assert!(stdout.contains("skip any-md scope=*.md"));
}

#[test]
fn explain_format_json_shape_is_stable() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(dir.path(), THREE_RULE_BODY);
    let file = dir.path().join("docs/intro.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "# hi\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "explain",
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
    // Parse-then-re-serialize so the snapshot is canonicalized; raw stdout
    // contains tempdir paths we don't want in the snapshot.
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    insta::assert_json_snapshot!("explain_md_file_json", v);
}

#[test]
fn explain_format_json_skipped_file_shape_is_stable() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(dir.path(), THREE_RULE_BODY);
    let lock = dir.path().join("Cargo.lock");
    std::fs::write(&lock, "# generated\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "explain",
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
    insta::assert_json_snapshot!("explain_lockfile_json", v);
}

#[test]
fn explain_missing_config_exits_one() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("foo.md");
    std::fs::write(&file, "x\n").unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "explain",
            "--config",
            dir.path().join(".hector.yml").to_str().unwrap(),
            file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ERROR"),
        "missing config must surface a stderr ERROR hint: {stderr}"
    );
}
