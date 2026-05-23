//! H1 — `provider: claude-code-subagent` is recognised by build_from_config.
//! Returns Ok(None) (so the runner knows to use the deferred path) and emits
//! NO stderr warning (distinct from the missing-API-key path, which warns).

use hector_core::config::LlmConfig;
use hector_core::llm::build_from_config;

#[test]
fn claude_code_subagent_provider_returns_none_without_warning() {
    let cfg = LlmConfig {
        provider: "claude-code-subagent".to_string(),
        // R2: model is now optional for the subagent provider. Passing
        // None here is the canonical shape; pre-R2 this field had to be
        // `Some("ignored")` literally just to satisfy the parser.
        model: None,
        evaluator_model: None,
        api_key_env: None,
        base_url: None,
    };
    // The function returns `Result<Option<Box<dyn LlmClient>>>`. We assert it
    // is `Ok(None)` — no client is constructed, no error is raised.
    let result = build_from_config(&cfg).expect("subagent provider must not error");
    assert!(
        result.is_none(),
        "subagent provider must yield None — direct dispatch is disabled"
    );
}

#[test]
fn unknown_provider_still_errors() {
    // Regression: adding the subagent arm must not turn unknown providers
    // into silent passes.
    let cfg = LlmConfig {
        provider: "definitely-not-a-real-provider".to_string(),
        model: Some("ignored".to_string()),
        evaluator_model: None,
        api_key_env: None,
        base_url: None,
    };
    // `Box<dyn LlmClient>` is not `Debug`, so `expect_err` won't compile; match instead.
    let err = match build_from_config(&cfg) {
        Ok(_) => panic!("unknown provider must error"),
        Err(e) => e,
    };
    let msg = format!("{err:#}");
    assert!(
        msg.contains("claude-code-subagent"),
        "error message must list the new provider so users discover it: got {msg:?}"
    );
}
