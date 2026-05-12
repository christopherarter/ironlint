use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Helper: write `body` to `.hector.yml` in `dir`, then append a valid
/// trust block via `hector trust`.
fn write_trusted_cfg(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let cfg = dir.join(".hector.yml");
    fs::write(&cfg, body).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    cfg
}

/// P2-12: a Block verdict from `hector check --session` must NOT clear
/// `.hector/session.json` — the user needs the session preserved to
/// re-inspect the offending edits. Only Pass and Warn explicitly
/// acknowledge a session and clear it.
///
/// We drive the binary end-to-end: a wiremock server returns a violation
/// for a single session rule, the binary produces Status::Block, and we
/// assert the session file is untouched on disk.
#[tokio::test(flavor = "multi_thread")]
async fn check_session_preserves_state_on_block() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [{
                "type": "text",
                "text": "[{\"rule_id\":\"audit-tests\",\"status\":\"violation\",\"message\":\"auth changed but no test file in session\"}]"
            }]
        })))
        .mount(&server)
        .await;
    let base_url = server.uri();

    // Use a uniquely-named env var to avoid races with parallel tests that
    // also set ANTHROPIC_API_KEY-style vars.
    let env_name = "HECTOR_TEST_BLOCK_PRESERVES";
    std::env::set_var(env_name, "fake-key");

    let dir = tempdir().unwrap();
    let cfg_body = format!(
        "schema_version: 2\nllm:\n  provider: anthropic\n  model: test-model\n  api_key_env: {env_name}\n  base_url: \"{base_url}\"\nrules:\n  audit-tests:\n    description: x\n    engine: session\n    scope: [\"src/**\"]\n    severity: error\n    context: repo\n"
    );
    let cfg = write_trusted_cfg(dir.path(), &cfg_body);

    let session_path = dir.path().join(".hector/session.json");
    fs::create_dir_all(dir.path().join(".hector")).unwrap();
    let body = r#"{"session_id":"s1","started_at":"t","edits":[{"file":"src/a.ts","diff":"+ const x = 1;","timestamp":"t"}]}"#;
    fs::write(&session_path, body).unwrap();

    // The subprocess HTTP call to wiremock is blocking; run it on a
    // worker thread so it doesn't block this tokio runtime.
    let cfg_str = cfg.to_str().unwrap().to_string();
    let session_path_clone = session_path.clone();
    let body_owned = body.to_string();
    tokio::task::spawn_blocking(move || {
        let output = Command::cargo_bin("hector")
            .unwrap()
            .args([
                "check",
                "--session",
                "--config",
                &cfg_str,
                "--format",
                "json",
            ])
            .assert()
            .code(2)
            .get_output()
            .clone();

        let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
        let v: Value = serde_json::from_str(&stdout).expect("parse verdict json");
        assert_eq!(v["status"], "block", "verdict json: {stdout}");

        assert!(
            session_path_clone.exists(),
            "session.json must persist on Block so the user can re-inspect"
        );
        let after = fs::read_to_string(&session_path_clone).unwrap();
        assert_eq!(
            after, body_owned,
            "session contents must be untouched on Block"
        );
    })
    .await
    .unwrap();

    std::env::remove_var(env_name);
}

#[test]
fn check_session_consumes_state() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(
        &cfg,
        "schema_version: 2\nrules:\n  noop:\n    description: x\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    ).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();

    fs::create_dir_all(dir.path().join(".hector")).unwrap();
    fs::write(
        dir.path().join(".hector/session.json"),
        r#"{"session_id":"s1","started_at":"t","edits":[{"file":"src/a.ts","diff":"+ const x = 1;","timestamp":"t"}]}"#,
    ).unwrap();

    let output = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--session",
            "--config",
            cfg.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let v: Value = serde_json::from_str(&stdout).expect("parse verdict json");
    assert_eq!(v["status"], "pass", "verdict json: {stdout}");

    assert!(!dir.path().join(".hector/session.json").exists());
}
