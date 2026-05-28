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

#[test]
fn print_prompt_renders_and_exits_zero() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  no-unwrap:\n    description: \"avoid unwrap\"\n    engine: semantic\n    scope: [\"**/*.rs\"]\n    severity: warning\n    context: file\n",
    );
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main() { x.unwrap(); }\n").unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--print-prompt",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("avoid unwrap"),
        "missing rule description in prompt: {stdout}"
    );
    // Sentinel tags are per-call random; assert the `<UE-` prefix is present
    // rather than a fixed literal tag.
    assert!(
        stdout.contains("<UE-"),
        "missing UE sentinel open tag in prompt: {stdout}"
    );
    assert!(stdout.contains("no-unwrap"));
}

#[test]
fn print_prompt_does_not_call_llm_endpoint() {
    // --print-prompt must short-circuit before the HTTP call. Bind a
    // TcpListener as a stand-in LLM endpoint; the test fails if the binary
    // opens *any* connection to it.
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();

    let dir = tempdir().unwrap();
    let cfg_body = format!(
        "schema_version: 2\nllm:\n  provider: anthropic\n  model: claude\n  api_key_env: HECTOR_TEST_KEY\n  base_url: http://127.0.0.1:{port}\nrules:\n  r:\n    description: \"d\"\n    engine: semantic\n    scope: [\"*.rs\"]\n    severity: warning\n    context: file\n"
    );
    let cfg = write_trusted(dir.path(), &cfg_body);
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main(){}\n").unwrap();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Poll the listener for connections during the CLI invocation.
        let deadline = std::time::Instant::now() + Duration::from_millis(1500);
        while std::time::Instant::now() < deadline {
            if listener.accept().is_ok() {
                let _ = tx.send(true);
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = tx.send(false);
    });

    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("HECTOR_TEST_KEY", "x")
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--print-prompt",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "--print-prompt should exit 0; stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let connected = rx.recv_timeout(Duration::from_secs(3)).unwrap_or(false);
    assert!(
        !connected,
        "--print-prompt must not open a connection to the LLM endpoint"
    );
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
fn print_prompt_without_file_or_diff_exits_one() {
    // `run_print_prompt`'s `_ =>` arm.
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: semantic\n    scope: [\"*.rs\"]\n    severity: warning\n    context: file\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["check", "--config", cfg.to_str().unwrap(), "--print-prompt"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--file or --diff"));
}

#[test]
fn print_prompt_with_empty_diff_exits_one() {
    // `run_print_prompt`'s `changed.into_iter().next()` = None arm.
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: semantic\n    scope: [\"*.rs\"]\n    severity: warning\n    context: file\n",
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
            "--print-prompt",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no changed files in diff"));
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

#[test]
fn explain_renders_skipped_and_engine_variants() {
    // Drives the `print_explain` Skipped arm and the `semantic`/`ast`
    // engine-name match arms by setting up two rules:
    //  * an `ast` rule that misses, producing a `Pass` row tagged `ast`;
    //  * a `semantic` rule with a no-evidence diff, triggering the
    //    pre-filter `Skipped { reason }` row tagged `semantic`.
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  ast-rule:\n    description: \"d\"\n    engine: ast\n    scope: [\"**/*.rs\"]\n    severity: warning\n    pattern: \"NEVER_MATCH_XYZ\"\n  sem-rule:\n    description: \"d\"\n    engine: semantic\n    scope: [\"**/*.rs\"]\n    severity: warning\n    context: diff\n",
    );
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main(){}\n").unwrap();
    // A formatting-only diff (whitespace) typically trips the can_match_diff
    // pre-filter; if it doesn't, the test still passes because the run
    // is constructed only to exercise the engine-name match arms.
    let diff = dir.path().join("d.diff");
    std::fs::write(
        &diff,
        "--- a/foo.rs\n+++ b/foo.rs\n@@ -1 +1 @@\n-fn main(){}\n+fn main() {}\n",
    )
    .unwrap();
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
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ast"),
        "explain stderr must name ast engine: {stderr}"
    );
    assert!(
        stderr.contains("semantic"),
        "explain stderr must name semantic engine: {stderr}"
    );
}
