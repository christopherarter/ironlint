//! LLM client trait + provider impls.

pub mod anthropic;
pub mod openai_compat;
pub mod prompt;

use crate::config::{LlmConfig, Rule};
use anyhow::{anyhow, bail, Result};
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

/// Maximum retries after the initial attempt (total attempts = 1 + this).
pub(crate) const MAX_LLM_RETRIES: u32 = 2;

/// HTTP status codes worth retrying: rate limits and transient upstream errors.
pub(crate) fn is_retryable_status(code: u16) -> bool {
    matches!(code, 429 | 500 | 502 | 503 | 504)
}

/// Exponential backoff: 250ms, 500ms, … for `attempt` 1, 2, … (no jitter —
/// single process, low concurrency).
pub(crate) fn backoff_delay(attempt: u32) -> std::time::Duration {
    let factor = 2u64.saturating_pow(attempt.saturating_sub(1));
    std::time::Duration::from_millis(250u64.saturating_mul(factor))
}

/// Run `send`, retrying up to `max_retries` times while `is_retryable` holds,
/// invoking `on_retry(attempt)` (e.g. to sleep) between attempts. Generic over
/// the result type so the loop is unit-testable without a real network call.
pub(crate) fn retry_with_backoff<T, E>(
    max_retries: u32,
    is_retryable: impl Fn(&std::result::Result<T, E>) -> bool,
    mut send: impl FnMut() -> std::result::Result<T, E>,
    mut on_retry: impl FnMut(u32),
) -> std::result::Result<T, E> {
    let mut attempt = 0u32;
    loop {
        let result = send();
        if attempt < max_retries && is_retryable(&result) {
            attempt += 1;
            on_retry(attempt);
            continue;
        }
        return result;
    }
}

/// Construct an `LlmClient` from a parsed config's `llm:` block.
///
/// Returns `Ok(None)` (with a stderr warning) when a non-Ollama provider
/// needs an API key but the configured env var is missing. This lets
/// non-LLM rules (script, ast) still run when credentials are absent.
///
/// Errors when the provider name is unknown.
pub fn build_from_config(cfg: &LlmConfig) -> Result<Option<Box<dyn LlmClient>>> {
    // The subagent path neither needs nor reads api_key_env. Returning early
    // before `read_api_key` avoids the "env var unset" stderr warning that
    // would otherwise fire for a user who copy-pasted an Anthropic config and
    // only switched `provider:`. Warn-once if they also supplied an explicit
    // `model:` — it's never read for this provider, and saying so at load time
    // heads off a "why isn't my model setting taking effect?" investigation.
    if cfg.provider == "claude-code-subagent" {
        warn_subagent_model_ignored(cfg.model.as_deref());
        return Ok(None);
    }
    // Direct-API providers still require `model:`. The error names the
    // provider so the diagnostic points at the right config field.
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
            // Handled by the early-return above so `read_api_key` never runs
            // for this provider (no spurious "env var unset" warning). Kept
            // for match exhaustiveness and so the bail arm's error message
            // stays accurate.
            unreachable!("handled by the early-return above");
        }
        other => {
            bail!("unknown LLM provider `{other}`. Supported: anthropic, claude-code-subagent, ollama, openrouter")
        }
    }
}

/// One-time per-process stderr warning when a user supplies
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

/// Side-effect-free probe used by `hector doctor`.
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

/// The conventional API-key env var for a direct-API provider, used when a
/// config omits `api_key_env`. Keeps the documented minimal config
///
/// ```yaml
/// llm:
///   provider: anthropic
///   model: claude-haiku-4-5
/// ```
///
/// working without an explicit `api_key_env:` line. Returns `None` for
/// providers that need no conventional default (`ollama` has no key;
/// `claude-code-subagent` never reaches `read_api_key`).
pub fn default_api_key_env(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        _ => None,
    }
}

fn read_api_key(cfg: &LlmConfig) -> Option<String> {
    let env_name = cfg
        .api_key_env
        .as_deref()
        .or_else(|| default_api_key_env(&cfg.provider))?;
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
/// inside an arbitrary string. Scrubs LLM-endpoint error bodies before they
/// bubble up through `anyhow`: a debug proxy or misconfigured server can echo
/// the caller's API key back in the response.
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

/// Find the balanced `[...]` span starting at byte index `start` (which must
/// point at a `[`), respecting JSON string literals so brackets inside strings
/// don't affect depth. Returns `None` if the array never closes.
fn balanced_array_span(s: &str, start: usize) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
        } else {
            match b {
                b'"' => in_str = true,
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        // start and i both index ASCII delimiters → valid bounds.
                        return Some(&s[start..=i]);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Pull the verdict array out of an LLM response that may wrap it in prose or
/// markdown. Tries each `[` as a candidate start and returns the first balanced
/// span that deserializes into the verdict shape — so incidental brackets like
/// `[see notes]` or `[1, 2, 3]` are skipped.
fn extract_wire_verdicts(text: &str) -> Result<Vec<WireVerdict>> {
    let trimmed = text.trim();
    let mut search_from = 0;
    while let Some(rel) = trimmed[search_from..].find('[') {
        let start = search_from + rel;
        if let Some(span) = balanced_array_span(trimmed, start) {
            if let Ok(wire) = serde_json::from_str::<Vec<WireVerdict>>(span) {
                return Ok(wire);
            }
        }
        search_from = start + 1;
    }
    Err(anyhow!(
        "no JSON verdict array found in response: {trimmed}"
    ))
}

/// Parse the LLM's JSON-array response into structured verdicts.
///
/// Tolerates prose or markdown fences around the array, and incidental
/// brackets before it, by scanning for the first balanced `[...]` that matches
/// the verdict shape.
pub fn parse_verdicts(text: &str) -> Result<Vec<RuleVerdict>> {
    let wire = extract_wire_verdicts(text)?;
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

#[cfg(test)]
mod parse_verdict_tests {
    use super::{parse_verdicts, RuleStatus};

    #[test]
    fn extracts_array_amid_prose_with_incidental_brackets() {
        let text = "I reviewed the changes [see the 2 notes] and conclude: \
                    [{\"rule_id\":\"r1\",\"status\":\"pass\"}] — all done [end]";
        let v = parse_verdicts(text).expect("must skip prose brackets and parse the real array");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rule_id, "r1");
        assert_eq!(v[0].status, RuleStatus::Pass);
    }

    #[test]
    fn skips_non_verdict_array_before_the_real_one() {
        let text = "scores: [1, 2, 3]. verdict: \
                    [{\"rule_id\":\"r2\",\"status\":\"violation\",\"message\":\"nope\",\"line\":4}]";
        let v = parse_verdicts(text).expect("must skip [1,2,3] and find the verdict array");
        assert_eq!(v.len(), 1);
        match &v[0].status {
            RuleStatus::Violation { message, line } => {
                assert_eq!(message, "nope");
                assert_eq!(*line, Some(4));
            }
            _ => panic!("expected violation"),
        }
    }

    #[test]
    fn plain_array_still_parses() {
        let v = parse_verdicts("[{\"rule_id\":\"r1\",\"status\":\"pass\"}]").unwrap();
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn no_array_is_an_error() {
        let err = parse_verdicts("the model refused to answer").expect_err("no array");
        assert!(format!("{err:#}").to_lowercase().contains("json"));
    }

    #[test]
    fn bracket_inside_json_string_does_not_end_the_span_early() {
        // A lone `]` inside a string value would, without the string-aware
        // scan, close the array early and corrupt the span. The in_str state
        // machine must ignore brackets inside string literals.
        let text = "[{\"rule_id\":\"r1\",\"status\":\"violation\",\"message\":\"unbalanced ] bracket\",\"line\":2}]";
        let v = parse_verdicts(text).expect("a `]` inside a string must not truncate the span");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rule_id, "r1");
        match &v[0].status {
            RuleStatus::Violation { message, line } => {
                assert!(message.contains("unbalanced ] bracket"));
                assert_eq!(*line, Some(2));
            }
            _ => panic!("expected violation"),
        }
    }

    #[test]
    fn unclosed_array_is_an_error_not_a_panic() {
        // A truncated response whose `[` never closes must return an Err
        // (balanced_array_span returns None → extraction finds no valid span),
        // not panic.
        let text = "verdict: [{\"rule_id\":\"r1\",\"status\":\"pass\"";
        let err = parse_verdicts(text).expect_err("unclosed array must error");
        assert!(format!("{err:#}").to_lowercase().contains("json"));
    }
}

#[cfg(test)]
mod retry_tests {
    use super::{is_retryable_status, retry_with_backoff};
    use std::cell::Cell;

    #[test]
    fn retries_until_non_retryable_then_returns_success() {
        let calls = Cell::new(0);
        let out: Result<u16, ()> = retry_with_backoff(
            2,
            |r| matches!(r, Ok(429)),
            || {
                calls.set(calls.get() + 1);
                if calls.get() < 3 {
                    Ok(429)
                } else {
                    Ok(200)
                }
            },
            |_attempt| {},
        );
        assert_eq!(out, Ok(200));
        assert_eq!(calls.get(), 3, "1 initial + 2 retries");
    }

    #[test]
    fn gives_up_after_max_retries_returning_last_result() {
        let calls = Cell::new(0);
        let out: Result<u16, ()> = retry_with_backoff(
            2,
            |r| matches!(r, Ok(429)),
            || {
                calls.set(calls.get() + 1);
                Ok(429)
            },
            |_| {},
        );
        assert_eq!(out, Ok(429));
        assert_eq!(calls.get(), 3, "no attempts beyond max_retries");
    }

    #[test]
    fn does_not_retry_on_first_success() {
        let calls = Cell::new(0);
        let out: Result<u16, ()> = retry_with_backoff(
            2,
            |r| matches!(r, Ok(429)),
            || {
                calls.set(calls.get() + 1);
                Ok(200)
            },
            |_| {},
        );
        assert_eq!(out, Ok(200));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn retryable_status_set() {
        for code in [429, 500, 502, 503, 504] {
            assert!(is_retryable_status(code), "{code} should retry");
        }
        for code in [200, 400, 401, 403, 404, 422] {
            assert!(!is_retryable_status(code), "{code} should not retry");
        }
    }
}
