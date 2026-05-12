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
