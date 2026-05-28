use hector_core::config::{EngineKind, Rule, Severity};
use hector_core::llm::anthropic::AnthropicClient;
use hector_core::llm::{LlmClient, RuleStatus};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_semantic_rule() -> Rule {
    Rule {
        description: "useEffect should not derive state from props".into(),
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
async fn anthropic_evaluate_returns_pass_for_clean_diff() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [{
                "type": "text",
                "text": "[{\"rule_id\":\"r1\",\"status\":\"pass\"}]"
            }]
        })))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let rule = make_semantic_rule();
    let result = tokio::task::spawn_blocking(move || {
        let client = AnthropicClient::new("test-key", "claude-sonnet-4-6", Some(base_url));
        client.evaluate(&[("r1", &rule)], "diff text", None)
    })
    .await
    .unwrap();
    let verdicts = result.expect("evaluate");
    assert_eq!(verdicts.len(), 1);
    assert_eq!(verdicts[0].status, RuleStatus::Pass);
}

#[tokio::test]
async fn anthropic_evaluate_returns_violation_with_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [{
                "type": "text",
                "text": "[{\"rule_id\":\"r1\",\"status\":\"violation\",\"message\":\"useEffect derives state from props\",\"line\":12}]"
            }]
        })))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let rule = make_semantic_rule();
    let result = tokio::task::spawn_blocking(move || {
        let client = AnthropicClient::new("test-key", "claude-sonnet-4-6", Some(base_url));
        client.evaluate(&[("r1", &rule)], "diff", None)
    })
    .await
    .unwrap();
    let verdicts = result.unwrap();
    match &verdicts[0].status {
        RuleStatus::Violation { message, line } => {
            assert!(message.contains("derives state from props"));
            assert_eq!(*line, Some(12));
        }
        _ => panic!("expected violation"),
    }
}

#[tokio::test]
async fn anthropic_returns_err_on_http_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(500).set_body_string("oops"))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let rule = make_semantic_rule();
    let result = tokio::task::spawn_blocking(move || {
        let client = AnthropicClient::new("test-key", "claude-sonnet-4-6", Some(base_url));
        client.evaluate(&[("r1", &rule)], "diff text", None)
    })
    .await
    .unwrap();
    let err = result.expect_err("expected HTTP 500 to surface as Err");
    let chain = format!("{:#}", err);
    assert!(
        chain.contains("500"),
        "expected error chain to mention 500, got: {chain}"
    );
}

#[tokio::test]
async fn anthropic_error_body_is_truncated_and_redacted() {
    // When the endpoint echoes a Bearer token or API key in its 5xx body, we
    // must not propagate the raw secret: truncate to ~200 chars and redact.
    let server = MockServer::start().await;
    let leaky = "error: Bearer sk-1234567890abcdef and more text".to_string() + &"x".repeat(500);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(500).set_body_string(leaky))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let rule = make_semantic_rule();
    let result = tokio::task::spawn_blocking(move || {
        let client = AnthropicClient::new("test-key", "claude-sonnet-4-6", Some(base_url));
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
async fn anthropic_returns_err_on_malformed_text_json() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [{
                "type": "text",
                "text": "not a json array"
            }]
        })))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let rule = make_semantic_rule();
    let result = tokio::task::spawn_blocking(move || {
        let client = AnthropicClient::new("test-key", "claude-sonnet-4-6", Some(base_url));
        client.evaluate(&[("r1", &rule)], "diff text", None)
    })
    .await
    .unwrap();
    let err = result.expect_err("expected malformed JSON to surface as Err");
    let chain = format!("{:#}", err).to_lowercase();
    assert!(
        chain.contains("parse") || chain.contains("json"),
        "expected error chain to mention parse or JSON, got: {chain}"
    );
}

#[tokio::test]
async fn anthropic_client_times_out_on_hung_request() {
    // A hung Anthropic endpoint must not block the entire check call. The
    // client builds with a 30s timeout; a 120s server delay must surface as an
    // Err in well under that, not block indefinitely.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(120)))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let rule = make_semantic_rule();
    let start = std::time::Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let client = AnthropicClient::new("test-key", "claude-sonnet-4-6", Some(base_url));
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
async fn anthropic_returns_err_on_missing_text_block() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": []
        })))
        .mount(&server)
        .await;
    let base_url = server.uri();
    let rule = make_semantic_rule();
    let result = tokio::task::spawn_blocking(move || {
        let client = AnthropicClient::new("test-key", "claude-sonnet-4-6", Some(base_url));
        client.evaluate(&[("r1", &rule)], "diff text", None)
    })
    .await
    .unwrap();
    let err = result.expect_err("expected missing text block to surface as Err");
    let chain = format!("{:#}", err);
    assert!(
        chain.contains("missing text content"),
        "expected error chain to mention 'missing text content', got: {chain}"
    );
}

#[tokio::test]
async fn anthropic_retries_once_on_429_then_succeeds() {
    // First call rate-limited, second succeeds. The client must retry and
    // return the verdict rather than surfacing the 429 as an error.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [{ "type": "text", "text": "[{\"rule_id\":\"r1\",\"status\":\"pass\"}]" }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let base_url = server.uri();
    let rule = make_semantic_rule();
    let result = tokio::task::spawn_blocking(move || {
        let client = AnthropicClient::new("test-key", "claude-sonnet-4-6", Some(base_url));
        client.evaluate(&[("r1", &rule)], "diff text", None)
    })
    .await
    .unwrap();
    let verdicts = result.expect("retry should recover from the 429");
    assert_eq!(verdicts.len(), 1);
    // Both mocks' `.expect(...)` are verified when `server` drops at scope end.
}
