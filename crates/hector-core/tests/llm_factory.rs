use hector_core::config::LlmConfig;
use hector_core::llm::{build_from_config, parse_verdicts, RuleStatus};
use hector_core::runner::{CheckInput, HectorEngine};
use hector_core::verdict::Status;
use std::fs;
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn lcfg(provider: &str, api_key_env: Option<&str>, base_url: Option<&str>) -> LlmConfig {
    LlmConfig {
        provider: provider.to_string(),
        model: "test-model".to_string(),
        api_key_env: api_key_env.map(str::to_string),
        base_url: base_url.map(str::to_string),
    }
}

#[test]
fn factory_anthropic_with_key_builds_client() {
    let env = "HECTOR_TEST_ANTHROPIC_OK";
    std::env::set_var(env, "fake-key");
    let cfg = lcfg("anthropic", Some(env), None);
    let result = build_from_config(&cfg).expect("ok");
    assert!(
        result.is_some(),
        "anthropic with key set should build client"
    );
    std::env::remove_var(env);
}

#[test]
fn factory_anthropic_missing_key_returns_none() {
    let env = "HECTOR_TEST_ANTHROPIC_MISSING";
    std::env::remove_var(env);
    let cfg = lcfg("anthropic", Some(env), None);
    let result = build_from_config(&cfg).expect("ok-with-warn");
    assert!(result.is_none(), "anthropic without key should return None");
}

#[test]
fn factory_ollama_works_without_key_or_base_url() {
    let cfg = lcfg("ollama", None, None);
    let result = build_from_config(&cfg).expect("ok");
    assert!(
        result.is_some(),
        "ollama should build with default base_url and no key"
    );
}

#[test]
fn factory_openrouter_with_key_builds_client() {
    let env = "HECTOR_TEST_OPENROUTER_OK";
    std::env::set_var(env, "fake-key");
    let cfg = lcfg("openrouter", Some(env), None);
    let result = build_from_config(&cfg).expect("ok");
    assert!(result.is_some());
    std::env::remove_var(env);
}

#[test]
fn factory_unknown_provider_errors() {
    let cfg = lcfg("groq", None, None);
    let err = match build_from_config(&cfg) {
        Ok(_) => panic!("expected unknown-provider error"),
        Err(e) => e,
    };
    let chain = format!("{err:#}");
    assert!(chain.contains("unknown LLM provider"), "msg: {chain}");
    assert!(chain.contains("anthropic"), "msg: {chain}");
    assert!(chain.contains("ollama"), "msg: {chain}");
    assert!(chain.contains("openrouter"), "msg: {chain}");
}

/// End-to-end proof that `HectorEngine::load` auto-wires the LLM from the
/// config's `llm:` block: we point `base_url` at a wiremock server and
/// confirm the semantic check produces a verdict driven by the mocked
/// response (rather than an "engine requires LlmClient" internal error).
#[tokio::test]
async fn hector_engine_load_auto_wires_llm_from_config() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [{
                "type": "text",
                "text": "[{\"rule_id\":\"r1\",\"status\":\"pass\"}]"
            }]
        })))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let env_name = "HECTOR_TEST_ENGINE_E2E";
    std::env::set_var(env_name, "fake-key");

    let dir = tempdir().unwrap();
    let cfg_body = format!(
        "schema_version: 2\nllm:\n  provider: anthropic\n  model: claude-sonnet-4-6\n  api_key_env: {env_name}\n  base_url: \"{base_url}\"\nrules:\n  r1:\n    description: \"semantic check\"\n    engine: semantic\n    scope: [\"*.tsx\"]\n    severity: error\n    context: file\n"
    );
    let cfg_path = dir.path().join(".hector.yml");
    fs::write(&cfg_path, &cfg_body).unwrap();
    let raw = fs::read_to_string(&cfg_path).unwrap();
    let with_trust = hector_core::trust::write_trust_block(&raw).unwrap();
    fs::write(&cfg_path, with_trust).unwrap();

    let file = dir.path().join("app.tsx");
    let content = "const X = () => null;\n";
    fs::write(&file, content).unwrap();

    let verdict = tokio::task::spawn_blocking(move || {
        let engine = HectorEngine::load(&cfg_path).expect("load");
        engine.check(CheckInput::File {
            path: file,
            content: content.into(),
        })
    })
    .await
    .unwrap()
    .unwrap();

    std::env::remove_var(env_name);

    assert_eq!(
        verdict.status,
        Status::Pass,
        "factory-wired LLM should drive Pass verdict, got: {verdict:?}"
    );
    assert!(
        verdict.passed_checks.contains(&"r1".to_string()),
        "passed_checks should contain r1: {:?}",
        verdict.passed_checks
    );
}

// ---- P1-5: parse_verdicts must error on unknown status, not silently emit
// a violation. Status matching is case-insensitive so models that emit
// "Pass" / "PASS" / "Violation" still parse.

#[test]
fn parse_verdicts_returns_err_on_unknown_status() {
    let body = r#"[{"rule_id": "r1", "status": "NEEDS_REVIEW"}]"#;
    let err =
        parse_verdicts(body).expect_err("unknown status must error, not yield a silent violation");
    let s = format!("{err:#}");
    assert!(
        s.contains("NEEDS_REVIEW") || s.contains("unknown"),
        "error must mention the bad status; got: {s}"
    );
    assert!(
        s.contains("r1"),
        "error should identify the offending rule id; got: {s}"
    );
}

#[test]
fn parse_verdicts_lowercases_status() {
    // "pass", "PASS", and "Pass" must all parse as RuleStatus::Pass after
    // case-insensitive matching. Likewise for "violation" / "VIOLATION".
    let body = r#"[
        {"rule_id":"r1","status":"pass"},
        {"rule_id":"r2","status":"PASS"},
        {"rule_id":"r3","status":"Pass"},
        {"rule_id":"r4","status":"Violation","message":"bad"},
        {"rule_id":"r5","status":"VIOLATION","message":"also bad","line":7}
    ]"#;
    let v = parse_verdicts(body).expect("all casings should parse");
    assert!(matches!(v[0].status, RuleStatus::Pass));
    assert!(matches!(v[1].status, RuleStatus::Pass));
    assert!(matches!(v[2].status, RuleStatus::Pass));
    assert!(matches!(v[3].status, RuleStatus::Violation { .. }));
    match &v[4].status {
        RuleStatus::Violation { message, line } => {
            assert_eq!(message, "also bad");
            assert_eq!(*line, Some(7));
        }
        other => panic!("expected Violation, got {other:?}"),
    }
}
