//! CLI integration tests for `--rule`, `--explain`, `--print-prompt`.

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

#[test]
fn rule_flag_restricts_evaluation_to_named_rule() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  keep:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n  drop:\n    description: \"y\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"exit 1\"\n",
    );
    let file = dir.path().join("ok.txt");
    std::fs::write(&file, "clean\n").unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--rule",
            "keep",
            "--format",
            "json",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let passed: Vec<&str> = v["passed_checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert!(passed.contains(&"keep"));
    assert!(!passed.contains(&"drop"));
}

#[test]
fn unknown_rule_id_exits_one_with_clear_error() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  keep:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    );
    let file = dir.path().join("ok.txt");
    std::fs::write(&file, "clean\n").unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--rule",
            "nope",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("nope"),
        "stderr must name the unknown rule id: {stderr}"
    );
}

#[test]
fn explain_prints_per_rule_outcome_to_stderr() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  pass-rule:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n  fire-rule:\n    description: \"y\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"exit 1\"\n",
    );
    let file = dir.path().join("foo.txt");
    std::fs::write(&file, "x\n").unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--explain",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("pass-rule"),
        "explain output missing pass-rule line: {stderr}"
    );
    assert!(
        stderr.contains("fire-rule"),
        "explain output missing fire-rule line: {stderr}"
    );
    assert!(stderr.contains("script"));
    assert!(stderr.contains("fire"));
    // JSON output on stdout must remain valid (no explain bleed-through).
    let _: serde_json::Value = serde_json::from_slice(&out.stdout)
        .expect("stdout JSON must remain parseable when --explain is on");
}

// ---------------------------------------------------------------------------
// Tests for `commands/check.rs` branches. Each exercises one arm of the CLI
// logic not reached by the happy-path tests above.
// ---------------------------------------------------------------------------

#[test]
fn check_without_file_or_diff_exits_one() {
    // The `_ => { ERROR: provide exactly one of --file or --diff }` arm.
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["check", "--config", cfg.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--file or --diff"),
        "stderr must guide the operator: {stderr}"
    );
}

#[test]
fn check_with_empty_diff_exits_one() {
    // `commands::check::run`'s `changed.is_empty()` arm for the non-explain
    // path.
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    );
    let diff = dir.path().join("empty.diff");
    std::fs::write(&diff, "").unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--diff",
            diff.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no changed files in diff"));
}

#[test]
fn missing_config_exits_one() {
    // First-load error path: `eprintln!("ERROR: ..."); return Ok(1);`.
    let dir = tempdir().unwrap();
    let bogus = dir.path().join("does-not-exist.yml");
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["check", "--config", bogus.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ERROR"));
}

#[test]
fn explain_with_diff_aggregates_rows() {
    // Drives `print_explain` with `--diff` (aggregated path) and exercises
    // the script engine-name branch.
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  pass-rule:\n    description: \"x\"\n    engine: script\n    scope: [\"**/*.txt\"]\n    severity: error\n    script: \"true\"\n",
    );
    // Write the post-edit file so the diff-mode read succeeds.
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n").unwrap();
    let diff = dir.path().join("d.diff");
    std::fs::write(&diff, "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-x\n+hello\n").unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--diff",
            diff.to_str().unwrap(),
            "--explain",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("pass-rule"),
        "diff-mode explain should print rows: {stderr}"
    );
}
