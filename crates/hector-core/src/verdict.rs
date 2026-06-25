use serde::{Deserialize, Serialize};

/// Verdict JSON schema version. Bumped to 4 for the gates redesign:
/// `Violation`/`Severity`/`Engine` removed; `blocks`/`errors` added.
pub const SCHEMA_VERSION: u32 = 4;

/// Floor schema version all current verdicts satisfy.
pub const MIN_REQUIRED_SCHEMA_VERSION: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub schema_version: u32,
    pub hector_version: String,
    pub status: Status,
    pub blocks: Vec<Block>,
    pub errors: Vec<GateError>,
    /// Gate ids that ran and passed (for `--explain` / telemetry).
    pub passed: Vec<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Status {
    Pass,
    Block,
    #[serde(rename = "internal_error")]
    InternalError,
}

/// A gate that exited 2 on a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub gate: String,
    pub file: String,
    /// Verbatim trimmed stdout+stderr from the gate.
    pub message: String,
}

/// A gate that crashed (not found / not executable / timeout / signal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateError {
    pub gate: String,
    pub file: String,
    /// Stable reason string from `InternalReason::as_str`.
    pub reason: String,
}

impl Verdict {
    pub fn pass() -> Self {
        Self::from_outcomes(vec![], vec![], vec![], 0)
    }

    /// Build a verdict from collected outcomes.
    ///
    /// Status precedence: **Block wins over InternalError** — a confirmed
    /// policy violation (exit 2) must stop the edit even if an unrelated gate
    /// crashed. Only when there are no blocks does a crash escalate to
    /// InternalError (exit 3, adapter fail-open).
    pub fn from_outcomes(
        blocks: Vec<Block>,
        errors: Vec<GateError>,
        passed: Vec<String>,
        elapsed_ms: u64,
    ) -> Self {
        let status = if !blocks.is_empty() {
            Status::Block
        } else if !errors.is_empty() {
            Status::InternalError
        } else {
            Status::Pass
        };
        Self {
            schema_version: SCHEMA_VERSION,
            hector_version: env!("CARGO_PKG_VERSION").to_string(),
            status,
            blocks,
            errors,
            passed,
            elapsed_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_pass() {
        let v = Verdict::from_outcomes(vec![], vec![], vec![], 0);
        assert_eq!(v.status, Status::Pass);
    }

    #[test]
    fn any_block_is_block() {
        let v = Verdict::from_outcomes(
            vec![Block {
                gate: "g".into(),
                file: "f".into(),
                message: "m".into(),
            }],
            vec![],
            vec![],
            0,
        );
        assert_eq!(v.status, Status::Block);
    }

    #[test]
    fn block_wins_over_internal_error() {
        let v = Verdict::from_outcomes(
            vec![Block {
                gate: "g".into(),
                file: "f".into(),
                message: "m".into(),
            }],
            vec![GateError {
                gate: "h".into(),
                file: "f".into(),
                reason: "timeout".into(),
            }],
            vec![],
            0,
        );
        assert_eq!(
            v.status,
            Status::Block,
            "a confirmed block must not be downgraded to fail-open by an unrelated crash"
        );
    }

    #[test]
    fn errors_only_is_internal_error() {
        let v = Verdict::from_outcomes(
            vec![],
            vec![GateError {
                gate: "h".into(),
                file: "f".into(),
                reason: "not_found".into(),
            }],
            vec![],
            0,
        );
        assert_eq!(v.status, Status::InternalError);
    }

    #[test]
    fn schema_version_is_4() {
        assert_eq!(SCHEMA_VERSION, 4);
    }
}
