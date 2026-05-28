//! `llm.model` is optional when `provider == claude-code-subagent`.
//!
//! The subagent path never reads `model:` — the in-session subagent inherits
//! the Claude Code session's model — so requiring it would force users to
//! type a placeholder. The invariants:
//!
//! * Configs with `provider: claude-code-subagent` and no `model:` field
//!   load successfully and `LlmConfig.model` is `None`.
//! * For all other providers, `model:` remains required — `load` errors
//!   if it is missing.
//! * `build_from_config` does not panic on `None` for the subagent arm.

use hector_core::config::{parse_str, LlmConfig};
use hector_core::llm::build_from_config;
use hector_core::runner::HectorEngine;
use std::fs;
use tempfile::tempdir;

#[test]
fn subagent_provider_loads_without_model_field() {
    // Subagent users do not need `model:` at all.
    let yaml = r#"schema_version: 2
llm:
  provider: claude-code-subagent
rules:
  r:
    description: "no DEBUG"
    engine: semantic
    scope: ["**/*.rs"]
    severity: warning
"#;
    let cfg = parse_str(yaml).expect("config without model: must parse under subagent provider");
    let llm = cfg.llm.expect("llm block present");
    assert_eq!(llm.provider, "claude-code-subagent");
    assert!(
        llm.model.is_none(),
        "model: omitted under subagent provider must deserialize to None, got {:?}",
        llm.model
    );
}

#[test]
fn subagent_provider_build_from_config_works_without_model() {
    // build_from_config must not unwrap cfg.model when the subagent arm fires.
    let cfg = LlmConfig {
        provider: "claude-code-subagent".to_string(),
        model: None,
        evaluator_model: None,
        api_key_env: None,
        base_url: None,
    };
    let result = build_from_config(&cfg).expect("subagent provider must not error on None model");
    assert!(result.is_none(), "subagent provider always yields None");
}

#[test]
fn non_subagent_provider_still_requires_model_at_load() {
    // HectorEngine::load enforces that direct-API providers carry a model.
    // The parser allows None (so we can detect-and-error with a clear message
    // instead of a serde "missing field" diagnostic), but load rejects it.
    let tmp = tempdir().unwrap();
    let yaml = r#"schema_version: 2
llm:
  provider: anthropic
  api_key_env: SOME_KEY
rules:
  r:
    description: "x"
    engine: script
    scope: ["**/*.rs"]
    severity: error
    script: "true"
"#;
    let path = tmp.path().join(".hector.yml");
    fs::write(&path, yaml).unwrap();
    let trusted =
        hector_core::trust::write_trust_block(&fs::read_to_string(&path).unwrap()).unwrap();
    fs::write(&path, trusted).unwrap();
    let err = match HectorEngine::load(&path) {
        Ok(_) => panic!("anthropic provider requires model:"),
        Err(e) => e,
    };
    let msg = format!("{err:#}");
    assert!(
        msg.contains("model") && msg.contains("anthropic"),
        "error must mention the missing `model:` field and the provider; got: {msg}"
    );
}
