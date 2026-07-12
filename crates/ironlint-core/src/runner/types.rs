use crate::telemetry::PerCheckRecord;
use crate::verdict::{Block, GateError, Status, Verdict};
use std::collections::HashSet;
use std::path::PathBuf;

/// Optional per-run knobs for [`IronLintEngine::check`]. Plumbed via
/// `builder().with_options(...)` so the public `check` signature stays stable
/// as knobs are added.
#[derive(Debug, Clone)]
pub struct CheckOptions {
    /// Restrict evaluation to these check ids. Empty set = run all checks. The
    /// filter is enforced before the check runs, so a filtered-out check never
    /// spawns a process.
    pub checks: HashSet<String>,
    /// What triggered this check, surfaced to checks as `$IRONLINT_EVENT`.
    pub event: String,
    /// Allow checking files whose canonical path falls outside `config_dir`.
    /// Off by default so wrappers can't run policy against arbitrary host
    /// files.
    pub allow_external_paths: bool,
    /// Suppress the `out_of_scope` skip for explicitly named (`checks`) ids, so
    /// an ad-hoc `--file` outside a check's glob still runs it. Scope-only:
    /// lifecycle and disable directives still apply.
    pub force: bool,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self {
            checks: HashSet::new(),
            event: "write".to_string(),
            allow_external_paths: false,
            force: false,
        }
    }
}

/// One row of the `--explain` report. Surfaced to the CLI via [`CheckReport`],
/// kept out of the verdict JSON (whose shape is locked).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckExplain {
    pub check_id: String,
    pub outcome: ExplainOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplainOutcome {
    /// Check exited 2 (blocked).
    Fire,
    /// Check ran and passed.
    Pass,
    /// Check did not run (filtered, out of scope, disabled) or crashed.
    Skipped { reason: String },
}

/// Companion return shape for [`IronLintEngine::check_with_explain`].
#[derive(Debug, Clone)]
pub struct CheckReport {
    pub verdict: Verdict,
    pub explain: Vec<CheckExplain>,
}

/// Input to a check. Checks evaluate the whole proposed file; the diff is
/// reconstructed by the caller when needed (CLI `--diff` enumerates changed
/// files into per-file `File` inputs).
pub enum CheckInput {
    File { path: PathBuf, content: String },
}

/// Per-check run result, before folding into the verdict. Skipped checks carry
/// a reason and contribute nothing to the verdict; ran checks carry their
/// wall-clock so telemetry can record it.
pub(crate) enum CheckStatus {
    Skipped(String),
    Pass(u64),
    Block {
        step: Option<String>,
        message: String,
        elapsed: u64,
    },
    Error {
        step: Option<String>,
        reason: String,
        detail: Option<String>,
        elapsed: u64,
    },
}

/// Folded outcomes across every check for one file.
#[derive(Default)]
pub(crate) struct Collected {
    pub(crate) blocks: Vec<Block>,
    pub(crate) errors: Vec<GateError>,
    pub(crate) passed: Vec<String>,
    pub(crate) records: Vec<PerCheckRecord>,
    pub(crate) explain: Vec<CheckExplain>,
}

impl Collected {
    /// Fold one check's status into the running totals. `collect_explain`
    /// gates the per-check explain row; skipped checks contribute only an
    /// explain row (no verdict entry, no telemetry record).
    ///
    /// `file` is `None` for set-level (pre-commit) invocations — the
    /// resulting `Block.file` / `GateError.file` will be `null` in the JSON.
    pub(crate) fn absorb(
        &mut self,
        check_id: &str,
        file: Option<&str>,
        status: CheckStatus,
        collect_explain: bool,
    ) {
        let outcome = match status {
            CheckStatus::Skipped(reason) => ExplainOutcome::Skipped { reason },
            CheckStatus::Pass(elapsed) => {
                self.passed.push(check_id.to_string());
                self.records.push(PerCheckRecord {
                    check: check_id.to_string(),
                    step: None,
                    status: Status::Pass,
                    elapsed_ms: elapsed,
                    reason: None,
                });
                ExplainOutcome::Pass
            }
            CheckStatus::Block {
                step,
                message,
                elapsed,
            } => {
                // Spec §5: a check that exits nonzero with no output still needs
                // a human-readable message. The check layer (`classify`) has no
                // check id, so it returns ""; we fill it in here where the id
                // and step name are both known.
                let message = if message.is_empty() {
                    match &step {
                        Some(name) => format!("{check_id} \u{203a} {name} blocked"),
                        None => format!("{check_id} blocked"),
                    }
                } else {
                    message
                };
                self.blocks.push(Block {
                    check: check_id.to_string(),
                    step: step.clone(),
                    file: file.map(|f| f.to_string()),
                    message,
                });
                self.records.push(PerCheckRecord {
                    check: check_id.to_string(),
                    step,
                    status: Status::Block,
                    elapsed_ms: elapsed,
                    reason: None,
                });
                ExplainOutcome::Fire
            }
            CheckStatus::Error {
                step,
                reason,
                detail,
                elapsed,
            } => {
                self.errors.push(GateError {
                    check: check_id.to_string(),
                    step: step.clone(),
                    file: file.map(|f| f.to_string()),
                    reason: reason.clone(),
                    detail: detail.clone(),
                });
                self.records.push(PerCheckRecord {
                    check: check_id.to_string(),
                    step,
                    status: Status::InternalError,
                    elapsed_ms: elapsed,
                    reason: Some(reason),
                });
                ExplainOutcome::Skipped {
                    reason: "engine_error".to_string(),
                }
            }
        };
        if collect_explain {
            self.explain.push(CheckExplain {
                check_id: check_id.to_string(),
                outcome,
            });
        }
    }
}
