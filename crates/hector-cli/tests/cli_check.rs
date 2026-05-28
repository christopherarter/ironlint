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
fn check_passing_rule_exits_zero() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("ok.txt");
    std::fs::write(&file, "clean\n").unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  noop:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(parsed["status"], "pass");
    let passed = parsed["passed_checks"]
        .as_array()
        .expect("passed_checks is an array");
    assert!(
        passed.iter().any(|v| v == "noop"),
        "expected `noop` in passed_checks, got {passed:?}"
    );
}

#[test]
fn check_blocking_rule_exits_two() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("bad.txt");
    std::fs::write(&file, "forbidden\n").unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  noforbidden:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"grep -q forbidden {file} && exit 1 || exit 0\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(parsed["status"], "block");
    assert_eq!(parsed["violations"][0]["rule_id"], "noforbidden");
}

#[test]
fn check_with_untrusted_config_exits_one() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("ok.txt");
    std::fs::write(&file, "clean\n").unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(
        &cfg,
        "schema_version: 2\nrules:\n  noop:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    ).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
        ])
        .assert()
        .code(1);
}

#[test]
fn check_with_relative_config_runs_script_in_config_dir() {
    // --config .hector.yml (bare filename, no directory component) makes
    // Path::parent() return Some("") rather than None. The runner must not
    // let config_dir collapse to "" and spawn the script engine with cwd="",
    // which would fail with ENOENT.
    let dir = tempdir().unwrap();
    let bad = dir.path().join("bad.txt");
    std::fs::write(&bad, "forbidden\n").unwrap();

    let cfg_path = dir.path().join(".hector.yml");
    std::fs::write(
        &cfg_path,
        "schema_version: 2\nrules:\n  noforbidden:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"grep -q forbidden {file} && exit 1 || exit 0\"\n",
    )
    .unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg_path.to_str().unwrap()])
        .assert()
        .success();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .args([
            "check",
            "--config",
            ".hector.yml",
            "--file",
            "bad.txt",
            "--format",
            "json",
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(parsed["status"], "block");
    // An "__internal" suffix would mean the script subprocess failed to spawn
    // (empty cwd). The real rule id must come through instead.
    assert_eq!(parsed["violations"][0]["rule_id"], "noforbidden");
}

#[test]
fn check_diff_input_parses_and_runs() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  noop:\n    description: \"x\"\n    engine: script\n    scope: [\"*.ts\"]\n    severity: error\n    script: \"true\"\n",
    );
    let patch = dir.path().join("change.patch");
    std::fs::write(
        &patch,
        "\
--- a/src/foo.ts
+++ b/src/foo.ts
@@ -1,3 +1,4 @@
 line one
-old line
+new line
+added line
 line three
",
    )
    .unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--diff",
            patch.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("valid json");
    let status = parsed["status"].as_str().expect("status is a string");
    assert!(
        matches!(status, "pass" | "warn" | "block"),
        "unexpected status: {status}"
    );
    assert_eq!(status, "pass");
}

// A unified diff with multiple changed files must check every file, not just
// the first `+++ b/` entry — violations in later files must not be dropped.
#[test]
fn cli_check_diff_processes_every_changed_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src/b.rs"), "fn b() { panic!(); }\n").unwrap();
    let cfg = write_trusted(
        root,
        "schema_version: 2\nrules:\n  no-panic:\n    description: x\n    engine: ast\n    language: rust\n    scope: [\"src/**/*.rs\"]\n    severity: error\n    pattern: panic!($$$)\n",
    );
    let diff = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-x\n+fn a() {}\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1 @@\n-x\n+fn b() { panic!(); }\n";
    let diff_path = root.join("multi.diff");
    std::fs::write(&diff_path, diff).unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--diff",
            diff_path.to_str().unwrap(),
            "--config",
            cfg.to_str().unwrap(),
            "--format",
            "json",
        ])
        .current_dir(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"rule_id\": \"no-panic\""),
        "violation must surface for src/b.rs: {stdout}"
    );
    // Exit 2 because the violation is Error severity → Block.
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn check_skips_cargo_lock_with_default_config() {
    let dir = tempdir().unwrap();
    // A script rule scoped to *.lock that always fails. If the rule ran
    // we'd see exit 2; if it's skipped we see exit 0. The skip is what
    // we're testing.
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  always-fail:\n    description: \"x\"\n    engine: script\n    scope: [\"*.lock\"]\n    severity: error\n    script: \"exit 1\"\n",
    );

    let lockfile = dir.path().join("Cargo.lock");
    std::fs::write(&lockfile, "# generated\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            lockfile.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(0);
}
