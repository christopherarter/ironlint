//! H1: deferred-evaluation envelope for the Claude Code subagent path.
//!
//! When `llm.provider: claude-code-subagent` (or `--emit-semantic-payload`)
//! is active and at least one `engine: semantic` or `engine: session` rule
//! survives scope/skip/diff-prefilter, the runner emits this envelope
//! instead of dispatching the rules. The Claude Code adapter's hook script
//! wraps `payload` in `hookSpecificOutput.additionalContext`; the
//! interpreter skill dispatches an in-session subagent against
//! `_evaluator_input`.
//!
//! Wire-format stability: changes to the shape MUST bump
//! [`DEFERRED_SCHEMA_VERSION`]. The shape is locked by an insta snapshot
//! in `tests/deferred_verdict_shape.rs`.

use serde::{Deserialize, Serialize};

/// Schema version for the deferred-evaluation envelope. Independent of
/// [`crate::verdict::SCHEMA_VERSION`] — the two schemas evolve separately.
///
/// **Policy (C6, 2026-05-25):** same additive-no-bump policy as
/// `SCHEMA_VERSION`. Bumps only on field removals, type changes, or
/// semantic re-interpretations. Additive fields with
/// `skip_serializing_if` do NOT bump.
///
/// History:
/// - v1: initial deferred-verdict shape (`schema_version`, `deferred`,
///   `hector_version`, `passed_checks`, `payload`, `elapsed_ms`).
/// - v2 (R5, 2026-05-23): new optional `payload.evaluator_model` field.
///   `#[serde(skip_serializing_if = "Option::is_none")]` keeps envelopes
///   without the field byte-compatible with the v1 shape, so existing
///   consumers that don't read the field do not break.
/// - v3 (B5, 2026-05-25): non-additive change to `_evaluator_input`. The
///   field now interpolates a per-call random sentinel
///   (`<TP-{32hex}>` / `<UE-{32hex}>`) instead of the literal
///   `<TRUSTED_POLICY>` / `<UNTRUSTED_EVIDENCE>` tags, and the rendered
///   prompt is built per-rule (each rule sees its declared `context:`
///   expansion) instead of sharing a single primary blob. Consumers
///   doing string-based extraction against the old tag names must update.
///   Additive: B4's `payload.warnings` (`skip_serializing_if =
///   "Vec::is_empty"`) — would not have bumped on its own.
pub const DEFERRED_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredVerdict {
    pub schema_version: u32,
    /// Always `true` for this envelope. The redundancy is intentional:
    /// downstream consumers can branch on a single top-level boolean
    /// without parsing `schema_version` or type discriminating against
    /// `Verdict`'s `status: pass | warn | block`.
    pub deferred: bool,
    pub hector_version: String,
    /// Rule IDs that ran-and-passed deterministically during this check.
    /// Mirrors `Verdict::passed_checks`; the subagent uses this to know
    /// what was already covered.
    pub passed_checks: Vec<String>,
    pub payload: DeferredPayload,
    pub elapsed_ms: u64,
}

/// The bully-shaped payload the Claude Code skill consumes. Field names
/// match bully byte-for-byte so the ported skill text needs no rewriting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredPayload {
    pub file: String,
    /// Unified diff (empty string for whole-file checks). The subagent
    /// uses this to identify changed regions when evaluating session
    /// rules; semantic rules see it as additional context.
    pub diff: String,
    pub passed_checks: Vec<String>,
    pub evaluate: Vec<DeferredRule>,
    /// The full evaluator prompt — system + user from
    /// [`crate::llm::prompt::build_evaluator_input`] — rendered as a
    /// single string. The skill passes this verbatim to the subagent.
    /// Field name uses an underscore prefix to match bully's wire format.
    #[serde(rename = "_evaluator_input")]
    pub evaluator_input: String,
    /// R5 (2026-05-23): optional subagent model override. When the user
    /// sets `llm.evaluator_model: <id>` in `.hector.yml` and
    /// `provider == claude-code-subagent`, the runner threads the value
    /// here so the Claude Code skill can surface it (today the skill
    /// reports the requested model so the operator can edit the
    /// `hector-evaluator` subagent's frontmatter — Claude Code's
    /// subagent dispatch does not accept an inline model override).
    /// `skip_serializing_if` keeps envelopes without the field
    /// byte-compatible with the v1 shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator_model: Option<String>,
    /// B4 (2026-05-25): warn-severity deterministic violations
    /// (script / AST rules with `severity: warning`) that would otherwise
    /// vanish from the CLI's deferred branch. The CLI suppresses the
    /// standard `Verdict` JSON when emitting a `DeferredVerdict`, so
    /// before B4 these violations were dropped entirely from stdout
    /// (still written to `.hector/log.jsonl`, but invisible to the
    /// operator and to the in-session subagent).
    ///
    /// Block-severity violations stay on `Verdict::violations` and
    /// suppress the deferred envelope entirely — see the CLI branch in
    /// `commands/check.rs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<DeferredWarning>,
}

/// A single warn-severity deterministic violation on a deferred envelope.
///
/// B4 (2026-05-25). Mirrors the shape of [`crate::verdict::Violation`]
/// but without `suggestion` / `context` (the subagent renders these
/// inline; the deferred channel is for recall, not display).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredWarning {
    pub rule_id: String,
    pub engine: crate::verdict::Engine,
    pub file: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredRule {
    pub id: String,
    pub description: String,
    /// `"error"` | `"warning"`. Stringly-typed in the wire format to
    /// match bully; converted from [`crate::config::Severity`] at
    /// payload-construction time.
    pub severity: String,
    /// `"semantic"` | `"session"`. Per spec §6 Q1, session rules are
    /// deferred under the same flag and identified here. The skill
    /// routes all entries through the same subagent regardless.
    pub engine: String,
}
