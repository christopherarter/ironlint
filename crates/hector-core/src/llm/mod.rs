//! LLM client trait + provider impls.

pub mod anthropic;
pub mod openai_compat;
pub mod prompt;

use crate::config::{LlmConfig, Rule};
use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

pub use anthropic::AnthropicClient;
pub use openai_compat::OpenAICompatClient;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleVerdict {
    pub rule_id: String,
    pub status: RuleStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleStatus {
    Pass,
    Violation { message: String, line: Option<u32> },
}

pub trait LlmClient: Send + Sync {
    fn evaluate(
        &self,
        rules: &[(&str, &Rule)],
        primary: &str,
        context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>>;
}

/// Construct an `LlmClient` from a parsed config's `llm:` block.
///
/// Returns `Ok(None)` (with a stderr warning) when a non-Ollama provider
/// needs an API key but the configured env var is missing. This lets
/// non-LLM rules (script, ast) still run when credentials are absent.
///
/// Errors when the provider name is unknown.
pub fn build_from_config(cfg: &LlmConfig) -> Result<Option<Box<dyn LlmClient>>> {
    // H1 short-circuit: the subagent path neither needs nor reads
    // api_key_env. Returning early before `read_api_key` avoids the
    // "env var unset" stderr warning that would otherwise fire for
    // a user who copy-pasted an Anthropic config and only switched
    // `provider:`.
    //
    // R2 (2026-05-23): also warn-once if the user supplied an explicit
    // `model:` value here — it's never read for this provider, and
    // surfacing that fact at load time saves a confused "why isn't my
    // model setting taking effect?" investigation.
    if cfg.provider == "claude-code-subagent" {
        warn_subagent_model_ignored(cfg.model.as_deref());
        return Ok(None);
    }
    // R2: direct-API providers still require `model:`. The error names
    // the provider so the diagnostic points at the right config field.
    let model = cfg.model.as_deref().ok_or_else(|| {
        anyhow!(
            "llm.model is required for provider `{}` (optional only for `claude-code-subagent`)",
            cfg.provider
        )
    })?;
    let api_key = read_api_key(cfg);
    match cfg.provider.as_str() {
        "anthropic" => {
            let Some(key) = api_key else {
                return Ok(None);
            };
            Ok(Some(Box::new(AnthropicClient::new(
                key,
                model,
                cfg.base_url.clone(),
            ))))
        }
        "openrouter" => {
            let Some(key) = api_key else {
                return Ok(None);
            };
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
            Ok(Some(Box::new(OpenAICompatClient::new(key, model, base))))
        }
        "ollama" => {
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434/v1".to_string());
            let key = api_key.unwrap_or_default();
            Ok(Some(Box::new(OpenAICompatClient::new(key, model, base))))
        }
        "claude-code-subagent" => {
            // H1 follow-up: handled by the early-return above so that
            // `read_api_key` never runs for this provider (no spurious
            // "env var unset" warning). This arm is kept for match
            // exhaustiveness signalling and so the bail arm's error
            // message stays accurate.
            unreachable!("handled by the early-return above");
        }
        other => {
            bail!("unknown LLM provider `{other}`. Supported: anthropic, claude-code-subagent, ollama, openrouter")
        }
    }
}

/// R2: one-time per-process stderr warning when a user supplies
/// `llm.model:` under `provider: claude-code-subagent`. The subagent
/// uses whatever model the parent Claude Code session is running, so
/// the value is silently ignored — surfacing that fact saves debugging.
///
/// Dedup pattern mirrors `engine::capability::should_warn_macos_with`:
/// an `AtomicBool` swap with `Ordering::Relaxed`. Correctness only
/// requires "fires at most once", no happens-before with surrounding
/// loads. Pulled into a free function so the swap target is reachable
/// from tests if we ever need to assert dedup.
fn warn_subagent_model_ignored(model: Option<&str>) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if model.is_none() {
        return;
    }
    if WARNED.swap(true, Ordering::Relaxed) {
        return;
    }
    eprintln!(
        "hector: llm.model is ignored when provider == claude-code-subagent \
         (the subagent uses the Claude Code session's model)"
    );
}

/// C1: side-effect-free probe used by `hector doctor`.
///
/// Reports whether the configured `api_key_env` env var is set to a
/// non-empty value, matching `read_api_key`'s emptiness rule (treats
/// the empty string as absent) so doctor reports the same answer the
/// runner would consult.
///
/// Returns `false` when the var is missing, unset, or empty. Never logs
/// (unlike `read_api_key`, which warns to stderr) — doctor builds its
/// own remediation message.
pub fn api_key_env_present(env_name: &str) -> bool {
    matches!(std::env::var(env_name), Ok(v) if !v.is_empty())
}

fn read_api_key(cfg: &LlmConfig) -> Option<String> {
    let env_name = cfg.api_key_env.as_deref()?;
    match std::env::var(env_name) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => {
            eprintln!(
                "hector: warning — env var `{env_name}` not set; provider `{}` requires it",
                cfg.provider
            );
            None
        }
    }
}

/// Maximum response-body slice (in *characters*, not bytes) we include in an
/// error message. A misbehaving endpoint can return megabytes; we only need
/// enough context to debug the failure.
const ERROR_BODY_CHAR_BUDGET: usize = 200;

/// Pre-compiled key/token patterns. ASCII-safe; `LazyLock` so we pay the
/// regex-build cost at most once per process.
static SECRET_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:sk|pk|api)-[A-Za-z0-9_-]{8,}").unwrap());
static BEARER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)bearer\s+[A-Za-z0-9_.\-]+").unwrap());

/// Mask common secret shapes (sk-/pk-/api- prefixed keys, `Bearer <token>`)
/// inside an arbitrary string. Used to scrub LLM-endpoint error bodies before
/// they bubble up through `anyhow` (P2-15): a debug proxy or misconfigured
/// server can echo the caller's API key back in the response.
pub(crate) fn redact_secrets(s: &str) -> String {
    let first = SECRET_KEY_RE.replace_all(s, "[REDACTED]");
    let second = BEARER_RE.replace_all(&first, "[REDACTED]");
    second.into_owned()
}

/// Truncate an error body to [`ERROR_BODY_CHAR_BUDGET`] chars (counted as
/// Unicode scalars, so we never split a multi-byte sequence) and then redact
/// any secret-like tokens inside the slice.
pub(crate) fn sanitize_error_body(body: &str) -> String {
    let truncated: String = body.chars().take(ERROR_BODY_CHAR_BUDGET).collect();
    redact_secrets(&truncated)
}

// ---- Wire-format helpers shared by Anthropic + OpenAI-compat clients ----

#[derive(Debug, Deserialize)]
struct WireVerdict {
    rule_id: String,
    status: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    line: Option<u32>,
}

/// Parse the LLM's JSON-array response into structured verdicts.
///
/// Tolerates extra prose around the array (some models wrap it in markdown
/// fences or a sentence).
pub fn parse_verdicts(text: &str) -> Result<Vec<RuleVerdict>> {
    let trimmed = text.trim();
    let start = trimmed
        .find('[')
        .ok_or_else(|| anyhow!("no JSON array in response: {trimmed}"))?;
    let end = trimmed
        .rfind(']')
        .ok_or_else(|| anyhow!("no closing bracket: {trimmed}"))?;
    let json = &trimmed[start..=end];
    let wire: Vec<WireVerdict> =
        serde_json::from_str(json).with_context(|| format!("parse verdict JSON: {json}"))?;
    let mut out = Vec::with_capacity(wire.len());
    for w in wire {
        let status = match w.status.to_ascii_lowercase().as_str() {
            "pass" => RuleStatus::Pass,
            "violation" => RuleStatus::Violation {
                message: w.message.unwrap_or_default(),
                line: w.line,
            },
            other => bail!(
                "unknown LLM status `{other}` for rule `{}`; expected `pass` or `violation`",
                w.rule_id
            ),
        };
        out.push(RuleVerdict {
            rule_id: w.rule_id,
            status,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod redact_tests {
    use super::{redact_secrets, sanitize_error_body, ERROR_BODY_CHAR_BUDGET};

    #[test]
    fn redacts_sk_prefixed_api_keys() {
        let out = redact_secrets("token=sk-1234567890abcdef trailing");
        assert!(!out.contains("sk-1234567890abcdef"), "got: {out}");
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_pk_and_api_prefixes() {
        let out = redact_secrets("pk-ABCDEFGHIJ and api-XYZ12345678");
        assert!(!out.contains("pk-ABCDEFGHIJ"));
        assert!(!out.contains("api-XYZ12345678"));
    }

    #[test]
    fn redacts_bearer_tokens_case_insensitive() {
        let out = redact_secrets("Authorization: BEARER abc.DEF-123_xyz");
        assert!(!out.to_lowercase().contains("bearer abc"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn leaves_innocuous_text_untouched() {
        let s = "no secrets here, just words";
        assert_eq!(redact_secrets(s), s);
    }

    #[test]
    fn sanitize_truncates_to_char_budget() {
        let huge = "x".repeat(5_000);
        let out = sanitize_error_body(&huge);
        assert_eq!(out.chars().count(), ERROR_BODY_CHAR_BUDGET);
    }

    #[test]
    fn sanitize_handles_multibyte_at_boundary() {
        // 220 "é" characters (2 bytes each). Truncation must respect char
        // boundaries — splitting a UTF-8 sequence would panic on `String::from`.
        let s: String = "é".repeat(220);
        let out = sanitize_error_body(&s);
        assert_eq!(out.chars().count(), ERROR_BODY_CHAR_BUDGET);
    }

    #[test]
    fn sanitize_truncates_then_redacts() {
        let leaky = format!("Bearer sk-supersecret-token {}", "x".repeat(500));
        let out = sanitize_error_body(&leaky);
        assert!(!out.contains("sk-supersecret-token"), "got: {out}");
        assert!(out.contains("[REDACTED]"));
        assert!(out.chars().count() <= ERROR_BODY_CHAR_BUDGET);
    }
}
