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
pub const DEFERRED_SCHEMA_VERSION: u32 = 1;

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
