use serde::{Deserialize, Serialize};

/// Verdict JSON schema version. Bumped to 5 for the checks pipeline redesign:
/// `Block.gate`→`check`, `GateError.gate`→`check`, both `file` fields
/// are nullable, and `step: Option<String>` added to both.
pub const SCHEMA_VERSION: u32 = 5;

/// Floor schema version all current verdicts satisfy.
pub const MIN_REQUIRED_SCHEMA_VERSION: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub schema_version: u32,
    pub ironlint_version: String,
    pub status: Status,
    pub blocks: Vec<Block>,
    pub errors: Vec<GateError>,
    /// Check ids that ran and passed (for `--explain` / telemetry).
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

/// A check that exited 2 on a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub check: String,
    /// Step within a multi-step check that blocked. `null` in Phase 1 (single
    /// `run`); populated in Phase 3 when `steps:` is introduced.
    pub step: Option<String>,
    /// File that triggered the block. `null` for run-once checks (e.g.
    /// `pre-commit` mode in Phase 4); always `Some` in Phase 1.
    pub file: Option<String>,
    /// Verbatim trimmed stdout+stderr from the check.
    pub message: String,
}

/// A check that crashed (not found / not executable / timeout / signal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateError {
    pub check: String,
    /// Step within a multi-step check that crashed. `null` in Phase 1.
    pub step: Option<String>,
    /// File under check. `null` for run-once checks.
    pub file: Option<String>,
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
    /// policy violation (exit 2) must stop the edit even if an unrelated check
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
            ironlint_version: env!("CARGO_PKG_VERSION").to_string(),
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
                check: "g".into(),
                step: None,
                file: Some("f".into()),
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
                check: "g".into(),
                step: None,
                file: Some("f".into()),
                message: "m".into(),
            }],
            vec![GateError {
                check: "h".into(),
                step: None,
                file: Some("f".into()),
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
                check: "h".into(),
                step: None,
                file: Some("f".into()),
                reason: "not_found".into(),
            }],
            vec![],
            0,
        );
        assert_eq!(v.status, Status::InternalError);
    }

    #[test]
    fn schema_version_is_5() {
        assert_eq!(SCHEMA_VERSION, 5);
    }

    /// Locks the full verdict-JSON wire shape: top-level keys, `Status`
    /// string casing, and `schema_version` (visible and literal — must show
    /// `5`). Covers a block, an internal error, and a passed entry in one
    /// verdict so every array shape is exercised. `elapsed_ms` and
    /// `ironlint_version` are redacted — the former is caller-supplied
    /// timing, the latter tracks `CARGO_PKG_VERSION` and would break this
    /// snapshot on every version bump otherwise.
    #[test]
    fn verdict_json_wire_shape() {
        let verdict = Verdict::from_outcomes(
            vec![Block {
                check: "no-todo".into(),
                step: Some("no-any".into()),
                file: Some("src/a.rs".into()),
                message: "TODO found".into(),
            }],
            vec![GateError {
                check: "flaky".into(),
                step: None,
                file: Some("src/b.rs".into()),
                reason: "not_found".into(),
            }],
            vec!["fmt".into()],
            1234,
        );
        insta::assert_json_snapshot!(verdict, {
            ".elapsed_ms" => "[ms]",
            ".ironlint_version" => "[version]",
        });
    }

    #[test]
    fn block_serializes_check_key_not_gate() {
        let b = Block {
            check: "rustfmt".into(),
            step: None,
            file: Some("a.rs".into()),
            message: "x".into(),
        };
        let j = serde_json::to_string(&b).unwrap();
        assert!(j.contains("\"check\":\"rustfmt\""), "{j}");
        assert!(!j.contains("\"gate\""), "{j}");
    }
}
