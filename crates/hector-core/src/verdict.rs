use serde::{Deserialize, Serialize};

/// Verdict JSON schema version.
///
/// Bumps ONLY on:
/// - field removals or type changes,
/// - enum variant removals,
/// - semantic re-interpretations of existing fields.
///
/// Additive changes (new optional field with `skip_serializing_if`,
/// new enum variant marked `#[non_exhaustive]`) do NOT bump. Consumers
/// wanting backward compatibility should read `MIN_REQUIRED_SCHEMA_VERSION`
/// and accept anything `>=`. Strict consumers reject any unexpected version.
///
/// v3 removed the `deferred_rules` field and the `semantic`/`session`
/// `Engine` variants when LLM evaluation was dropped.
pub const SCHEMA_VERSION: u32 = 3;

/// Floor schema version that all current verdicts satisfy.
///
/// Consumers should assert `schema_version >= MIN_REQUIRED_SCHEMA_VERSION`
/// rather than `schema_version == <constant>`, so they remain compatible
/// with future additive bumps.
pub const MIN_REQUIRED_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub schema_version: u32,
    pub hector_version: String,
    pub status: Status,
    pub violations: Vec<Violation>,
    pub passed_checks: Vec<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Status {
    Pass,
    Warn,
    Block,
    /// At least one rule failed to evaluate due to an engine-internal error
    /// (AST refused diff, script spawn failure). Surfaces in
    /// `Violation::engine = Internal` rows.
    /// CLI maps to exit code 3 so adapters can distinguish "config
    /// wrong" from "policy violated" (exit 2).
    #[serde(rename = "internal_error")]
    InternalError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub rule_id: String,
    pub severity: Severity,
    pub engine: Engine,
    pub file: String,
    pub line: Option<u32>,
    /// 1-based column of the violation's start position.
    ///
    /// Only the AST engine populates this — it reads the column from the
    /// matched node's start byte. The `script` engine has no positional
    /// information from a regex hit and always leaves this `None`.
    pub column: Option<u32>,
    pub message: String,
    pub suggestion: Option<String>,
    /// Snippet of source surrounding the violation.
    ///
    /// AST populates this with the matched node's line ±3 lines for editor
    /// display. The script engine leaves it `None`.
    pub context: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Script,
    Ast,
    /// True trust-gate failure: config fingerprint mismatch.
    ///
    /// In practice trust failures halt at `HectorEngine::load`, so this
    /// variant is rarely seen in a `Violation`. Reserved for the case
    /// where a downstream caller wants to surface a trust-rejection as a
    /// structured verdict instead of a load error.
    Trust,
    /// Engine-internal runtime error (AST refused diff, script spawn
    /// failure, etc.). The rule's `rule_id` is suffixed with `__internal`
    /// by the runner so consumers can distinguish runtime errors from rule
    /// violations.
    Internal,
}

impl Verdict {
    pub fn pass() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            hector_version: env!("CARGO_PKG_VERSION").to_string(),
            status: Status::Pass,
            violations: vec![],
            passed_checks: vec![],
            elapsed_ms: 0,
        }
    }

    pub fn from_violations(
        violations: Vec<Violation>,
        passed: Vec<String>,
        elapsed_ms: u64,
    ) -> Self {
        let has_internal = violations.iter().any(|v| v.engine == Engine::Internal);
        let has_error = violations
            .iter()
            .any(|v| v.engine != Engine::Internal && v.severity == Severity::Error);
        let status = if has_internal {
            Status::InternalError
        } else if has_error {
            Status::Block
        } else if violations.is_empty() {
            Status::Pass
        } else {
            Status::Warn
        };
        Self {
            schema_version: SCHEMA_VERSION,
            hector_version: env!("CARGO_PKG_VERSION").to_string(),
            status,
            violations,
            passed_checks: passed,
            elapsed_ms,
        }
    }
}
