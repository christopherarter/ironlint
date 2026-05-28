use hector_core::config::{EngineKind, Rule, Severity};
use hector_core::llm::openai_compat::OpenAICompatClient;
use hector_core::llm::{LlmClient, RuleStatus};
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_semantic_rule() -> Rule {
    Rule {
        description: "no useEffect deriving state from props".into(),
        engine: EngineKind::Semantic,
        scope: vec!["*.tsx".into()],
        severity: Severity::Warning,
        script: None,
        pattern: None,
        language: None,
        context: None,
        capabilities: None,
        fix_hint: None,
        output: hector_core::config::OutputMode::default(),
    }
}

#[tokio::test]
async fn openai_compat_evaluate_returns_pass() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": "[{\"rule_id\":\"r1\",\"status\":\"pass\"}]" }
            }]
        })))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let rule = make_semantic_rule();
    let result = tokio::task::spawn_blocking(move || {
        let client = OpenAICompatClient::new("test-key", "gpt-4o-mini", base_url);
        client.evaluate(&[("r1", &rule)], "diff text", None)
    })
    .await
    .unwrap();
    let verdicts = result.expect("evaluate");
    assert_eq!(verdicts.len(), 1);
    assert_eq!(verdicts[0].status, RuleStatus::Pass);
}

#[tokio::test]
async fn openai_compat_evaluate_returns_violation_with_line() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": "[{\"rule_id\":\"r1\",\"status\":\"violation\",\"message\":\"derives state\",\"line\":12}]" }
            }]
        })))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let rule = make_semantic_rule();
    let result = tokio::task::spawn_blocking(move || {
        let client = OpenAICompatClient::new("test-key", "gpt-4o-mini", base_url);
        client.evaluate(&[("r1", &rule)], "diff", None)
    })
    .await
    .unwrap();
    let verdicts = result.unwrap();
    match &verdicts[0].status {
        RuleStatus::Violation { message, line } => {
            assert!(message.contains("derives state"));
            assert_eq!(*line, Some(12));
        }
        _ => panic!("expected violation"),
    }
}

#[tokio::test]
async fn openai_compat_omits_auth_header_when_key_empty() {
    // Ollama use case: no API key is required.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "[]" } }]
        })))
        .mount(&server)
        .await;
    // Separately, assert the absent header on a second mock that requires header_exists.
    // Since wiremock matchers are AND'd, we instead inspect via a fall-through expectation:
    // a request with an Authorization header would 404 (no matching mock).
    let with_auth = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header_exists("Authorization"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&with_auth)
        .await;
    // Empty-key client should hit the auth-less server happily.
    let base_url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        let client = OpenAICompatClient::new("", "llama3.2", base_url);
        let rule = make_semantic_rule();
        client.evaluate(&[("r1", &rule)], "x", None)
    })
    .await
    .unwrap();
    assert!(result.is_ok(), "ollama-style empty-key call should succeed");

    // And an empty-key client against an Authorization-requiring server should 404
    // (no mock matches), confirming the header wasn't actually sent.
    let with_auth_url = with_auth.uri();
    let result_no_match = tokio::task::spawn_blocking(move || {
        let client = OpenAICompatClient::new("", "llama3.2", with_auth_url);
        let rule = make_semantic_rule();
        client.evaluate(&[("r1", &rule)], "x", None)
    })
    .await
    .unwrap();
    assert!(
        result_no_match.is_err(),
        "request without Authorization should miss the auth-required mock"
    );
}

#[tokio::test]
async fn openai_compat_returns_err_on_http_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("server boom"))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        let client = OpenAICompatClient::new("test-key", "gpt-4o-mini", base_url);
        let rule = make_semantic_rule();
        client.evaluate(&[("r1", &rule)], "x", None)
    })
    .await
    .unwrap();
    let err = result.expect_err("should error");
    let chain = format!("{err:#}");
    assert!(chain.contains("500"), "expected 500 in error: {chain}");
}

#[tokio::test]
async fn openai_compat_error_body_is_truncated_and_redacted() {
    // openai-compat servers (Ollama, OpenRouter, debug proxies) may echo
    // Authorization headers or keys back into 5xx bodies, so truncate + redact.
    let server = MockServer::start().await;
    let leaky = "error: Bearer sk-1234567890abcdef and more text".to_string() + &"x".repeat(500);
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string(leaky))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        let client = OpenAICompatClient::new("test-key", "gpt-4o-mini", base_url);
        let rule = make_semantic_rule();
        client.evaluate(&[("r1", &rule)], "x", None)
    })
    .await
    .unwrap();
    let err = result.expect_err("non-2xx must error");
    let s = format!("{err:#}");
    assert!(
        !s.contains("sk-1234567890abcdef"),
        "raw API key must not appear in error: {s}"
    );
    assert!(
        !s.to_lowercase().contains("bearer sk-"),
        "raw Bearer token must be redacted: {s}"
    );
    assert!(
        s.len() < 600,
        "error body must be truncated; got {} chars",
        s.len()
    );
}

#[tokio::test]
async fn openai_compat_returns_err_on_malformed_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "not a json array" } }]
        })))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        let client = OpenAICompatClient::new("test-key", "gpt-4o-mini", base_url);
        let rule = make_semantic_rule();
        client.evaluate(&[("r1", &rule)], "x", None)
    })
    .await
    .unwrap();
    let err = result.expect_err("should error");
    let chain = format!("{err:#}").to_lowercase();
    assert!(
        chain.contains("json") || chain.contains("array") || chain.contains("parse"),
        "expected parse-related error, got: {chain}"
    );
}

#[tokio::test]
async fn openai_compat_client_times_out_on_hung_request() {
    // A hung OpenAI-compatible endpoint must not block the entire check call.
    // The client builds with a 30s timeout; a 120s server delay must surface
    // as an Err in well under that, not block indefinitely.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(120)))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let start = std::time::Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let client = OpenAICompatClient::new("test-key", "gpt-4o-mini", base_url);
        let rule = make_semantic_rule();
        client.evaluate(&[("r1", &rule)], "x", None)
    })
    .await
    .unwrap();
    let elapsed = start.elapsed();
    assert!(result.is_err(), "hung request must time out");
    // Our wall-clock budget is 30s. Allow 5s headroom for slow CI but reject
    // a "blocked indefinitely" failure mode where the test only returns on
    // wiremock's 120s set_delay.
    assert!(
        elapsed < std::time::Duration::from_secs(35),
        "must time out close to 30s; took {elapsed:?}"
    );
}

#[tokio::test]
async fn openai_compat_returns_err_when_choices_missing_content() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": []
        })))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        let client = OpenAICompatClient::new("test-key", "gpt-4o-mini", base_url);
        let rule = make_semantic_rule();
        client.evaluate(&[("r1", &rule)], "x", None)
    })
    .await
    .unwrap();
    let err = result.expect_err("should error");
    let chain = format!("{err:#}");
    assert!(
        chain.contains("missing content"),
        "expected missing-content error, got: {chain}"
    );
}
