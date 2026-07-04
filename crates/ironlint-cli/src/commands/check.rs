use crate::cli::OutputFormat;
use crate::commands::error_report::emit_error;
use anyhow::{Context, Result};
use ironlint_core::runner::{
    CheckExplain, CheckInput, CheckOptions, ExplainOutcome, IronLintEngine,
};
use ironlint_core::trust::TrustOutcome;
use ironlint_core::verdict::{Status, Verdict};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The POSIX shell the engine spawns checks through (`engine/gate.rs`).
pub(crate) const POSIX_SHELL: &str = "sh";

/// Probes whether a given command can be executed via `Command::new(cmd).arg
/// ("-c").arg("exit 0").status()`. Returns `false` on `ErrorKind::NotFound` or
/// any other spawn failure (permission denied, not executable). Kept simple
/// to stay well under the cognitive-complexity cap.
///
/// `doctor` calls this with `POSIX_SHELL`; tests probe a guaranteed-absent name.
pub(crate) fn shell_available(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("-c")
        .arg("exit 0")
        .status()
        .map(|_| true)
        .unwrap_or(false)
}

/// Message printed (to stderr) and exit-1'd at the top of `check::run` when no
/// POSIX shell is on PATH. Exit 1 (config tier), NOT 3 — 3 is InternalError,
/// which adapters fail-open on; a missing shell must fail loud, never silent.
const NO_SHELL_MSG: &str = "no POSIX shell (`sh`) found on PATH. IronLint runs checks via `sh -c`\
     \nand cannot enforce anything without it. On Windows, run IronLint inside \
     \nGit Bash or WSL. See docs/getting-started.md.";

#[allow(clippy::too_many_arguments)]
#[allow(clippy::fn_params_excessive_bools)]
pub fn run(
    file: Option<PathBuf>,
    diff: Option<PathBuf>,
    content: Option<String>,
    format: OutputFormat,
    config: &Path,
    checks: Vec<String>,
    event: String,
    explain: bool,
    allow_external_paths: bool,
    force: bool,
    require_match: bool,
) -> Result<i32> {
    let config = match crate::commands::config::resolve_config(config) {
        Ok(p) => p,
        Err(msg) => return Ok(emit_error(format, &msg, 1)),
    };
    let config = config.as_path();
    if force && checks.is_empty() {
        return Ok(emit_error(
            format,
            "--force requires at least one --check <id>",
            1,
        ));
    }
    // Fail loud when the POSIX shell the engine spawns is absent (stock
    // Windows). Without `sh` every check fails to spawn → exit 3 → adapters
    // fail open → the user "enforces" nothing. We surface this as a config-tier
    // exit 1 (not 3) so nobody is fooled into thinking enforcement is active.
    if !shell_available(POSIX_SHELL) {
        return Ok(emit_error(format, NO_SHELL_MSG, 1));
    }
    // Trust gate: refuse an unblessed or tampered config/checks before the engine
    // loads or any check runs. This hashes the config + `.ironlint/gates/` now; a
    // write between here and check execution is a known, accepted TOCTOU window
    // (the direnv-model limitation — no file locking in 0.4).
    //
    // Exit code split (Task 3.2 / Finding C3, a sanctioned extension of the
    // locked 0/1/2/3 contract): a genuinely untrusted/tampered config gets
    // its OWN exit code, 4, distinct from exit 1 (config/parse error) — every
    // adapter previously mapped exit 1 to allow, so an untrusted config was
    // silently un-gated. A config the trust layer can't even evaluate (parse
    // error, missing extends target, ...) is not a trust decision at all; it
    // keeps exit 1 and is left for `engine.load` below to report on its own
    // terms.
    match ironlint_core::trust::check_trust(config) {
        Ok(TrustOutcome::Trusted) => {}
        Ok(TrustOutcome::Untrusted(e)) => {
            return Ok(emit_error(format, &format!("{e:#}"), 4));
        }
        Ok(TrustOutcome::Unverifiable(e)) | Err(e) => {
            return Ok(emit_error(format, &format!("{e:#}"), 1));
        }
    }
    let options = CheckOptions {
        checks: HashSet::new(),
        event,
        allow_external_paths,
        force,
    };
    let mut engine = match IronLintEngine::builder().with_options(options).load(config) {
        Ok(e) => e,
        Err(e) => return Ok(emit_error(format, &format!("{e:#}"), 1)),
    };
    if let Some(code) = validate_check_filter(&engine, &checks, format) {
        return Ok(code);
    }
    engine.set_check_filter(checks.into_iter().collect());

    match (file, diff) {
        (Some(f), None) => run_file(&engine, f, content, format, explain, require_match),
        (None, Some(d)) => run_diff(&engine, &d, format, explain, require_match),
        _ => Ok(emit_error(
            format,
            "provide exactly one of --file or --diff",
            1,
        )),
    }
}

fn run_file(
    engine: &IronLintEngine,
    file: PathBuf,
    content: Option<String>,
    format: OutputFormat,
    explain: bool,
    require_match: bool,
) -> Result<i32> {
    let content = match content {
        Some(c) => resolve_content_value(c)?,
        None => std::fs::read_to_string(&file)
            .with_context(|| format!("failed to read {}", file.display()))?,
    };
    let report = match engine.check_with_explain(CheckInput::File {
        path: file,
        content,
    }) {
        Ok(r) => r,
        Err(e) => {
            // e.g. an external path resolving outside config_dir: an
            // argument/config error, so exit 1 (mirrors the load-error path).
            return Ok(emit_error(format, &format!("{e:#}"), 1));
        }
    };
    if explain {
        print_explain(&report.explain);
    }
    emit(&report.verdict, format)?;
    Ok(exit_code(&report.verdict, require_match))
}

/// Check every non-deleted changed file in a unified diff. Checks read each
/// file's current on-disk content (checks don't consume diffs).
///
/// When `event == "pre-commit"` the engine's `check_set` is called once over
/// the full set of changed paths (run-once semantics). For all other events
/// the per-file loop runs each file through `check_with_explain` individually.
fn run_diff(
    engine: &IronLintEngine,
    diff: &Path,
    format: OutputFormat,
    explain: bool,
    require_match: bool,
) -> Result<i32> {
    let unified = std::fs::read_to_string(diff)?;
    let changed = ironlint_core::diff::parser::parse_unified(&unified)?;
    if changed.is_empty() {
        return Ok(emit_error(format, "no changed files in diff", 1));
    }
    let non_deleted: Vec<_> = changed
        .iter()
        .filter(|f| f.op != ironlint_core::diff::ChangeOp::Deleted)
        .collect();

    // Pre-commit: run each check once over the entire changed set.
    if engine.event() == "pre-commit" {
        let paths: Vec<PathBuf> = non_deleted.iter().map(|f| f.path.clone()).collect();
        let verdict = engine.check_set(&paths)?;
        emit(&verdict, format)?;
        return Ok(exit_code(&verdict, require_match));
    }

    // Write (and any future per-file event): loop once per changed file.
    let mut blocks = Vec::new();
    let mut errors = Vec::new();
    let mut passed = Vec::new();
    let mut explains: Vec<CheckExplain> = Vec::new();
    let mut elapsed = 0u64;
    for f in non_deleted {
        // A changed file we can't read (deleted between diff-gen and check,
        // permissions, non-UTF-8 bytes) is a SKIP, not a hard error:
        // fabricating empty content would run every check against "" and let
        // a real violation pass vacuously, but aborting the whole batch would
        // hide a real Block in a sibling file. Record the skip, warn loudly,
        // and move on — the aggregate verdict is still computed honestly from
        // every file that *could* be read.
        let content = match read_changed_file(&f.path) {
            Ok(c) => c,
            Err(reason) => {
                let label = reason.label();
                eprintln!(
                    "WARNING: skipping changed file {} ({label}): {reason}",
                    f.path.display()
                );
                explains.push(CheckExplain {
                    check_id: f.path.display().to_string(),
                    outcome: ExplainOutcome::Skipped {
                        reason: label.to_string(),
                    },
                });
                continue;
            }
        };
        let r = match engine.check_with_explain(CheckInput::File {
            path: f.path.clone(),
            content,
        }) {
            Ok(r) => r,
            Err(e) => {
                return Ok(emit_error(format, &format!("{e:#}"), 1));
            }
        };
        elapsed = elapsed.saturating_add(r.verdict.elapsed_ms);
        blocks.extend(r.verdict.blocks);
        errors.extend(r.verdict.errors);
        passed.extend(r.verdict.passed);
        explains.extend(r.explain);
    }
    let verdict = Verdict::from_outcomes(blocks, errors, passed, elapsed);
    if explain {
        print_explain(&explains);
    }
    emit(&verdict, format)?;
    Ok(exit_code(&verdict, require_match))
}

/// Why a diff-referenced file couldn't be turned into check input. Kept
/// separate from `Skipped`/`Fire`/`Pass` in [`ExplainOutcome`] — a read
/// failure happens before the engine ever sees the file, so it's classified
/// here and folded into the engine's existing skip vocabulary at the call
/// site rather than growing a new one.
enum SkipReason {
    /// `read_to_string` failed with `ErrorKind::InvalidData` — the file's
    /// bytes are not valid UTF-8 (image, UTF-16, other binary fixture).
    NonUtf8,
    /// Any other read failure (deleted between diff-gen and check,
    /// permissions, ...). Carries the io error kind for the stderr note.
    Unreadable(std::io::ErrorKind),
}

impl SkipReason {
    /// Stable, matchable reason string — surfaced in both the stderr warning
    /// and the `ExplainOutcome::Skipped { reason }` row.
    fn label(&self) -> &'static str {
        match self {
            Self::NonUtf8 => "non_utf8",
            Self::Unreadable(_) => "unreadable",
        }
    }
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonUtf8 => write!(f, "not valid UTF-8"),
            Self::Unreadable(kind) => write!(f, "could not be read: {kind}"),
        }
    }
}

/// Read a diff-referenced file's on-disk content, classifying any failure
/// into a [`SkipReason`] instead of a hard error. Extracted so the `run_diff`
/// loop body stays under the cognitive-complexity cap.
fn read_changed_file(path: &Path) -> Result<String, SkipReason> {
    std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::InvalidData {
            SkipReason::NonUtf8
        } else {
            SkipReason::Unreadable(e.kind())
        }
    })
}

fn validate_check_filter(
    engine: &IronLintEngine,
    checks: &[String],
    format: OutputFormat,
) -> Option<i32> {
    if checks.is_empty() {
        return None;
    }
    let known: HashSet<&str> = engine.check_ids().collect();
    let unknown: Vec<&str> = checks
        .iter()
        .map(|s| s.as_str())
        .filter(|id| !known.contains(id))
        .collect();
    if unknown.is_empty() {
        None
    } else {
        Some(emit_error(
            format,
            &format!("unknown check id(s): {}", unknown.join(", ")),
            1,
        ))
    }
}

fn print_explain(rows: &[CheckExplain]) {
    for row in rows {
        let outcome = match &row.outcome {
            ExplainOutcome::Fire => "fire".to_string(),
            ExplainOutcome::Pass => "pass".to_string(),
            ExplainOutcome::Skipped { reason } => format!("skipped {reason}"),
        };
        eprintln!("{} {}", row.check_id, outcome);
    }
}

fn exit_code(v: &Verdict, require_match: bool) -> i32 {
    let no_match = v.status == Status::Pass
        && v.passed.is_empty()
        && v.blocks.is_empty()
        && v.errors.is_empty();
    match v.status {
        Status::Block => 2,
        Status::InternalError => 3,
        Status::Pass if no_match && require_match => 2,
        _ => 0,
    }
}

fn emit(v: &Verdict, format: OutputFormat) -> Result<()> {
    let no_match = v.status == Status::Pass
        && v.passed.is_empty()
        && v.blocks.is_empty()
        && v.errors.is_empty();
    match format {
        OutputFormat::Json => {
            // JSON mode: no extra stdout (the verdict already carries
            // passed=[] for a no-match run; CI detects it from the shape).
            // --require-match still affects the EXIT code, handled by the
            // caller via exit_code(v, require_match).
            println!("{}", serde_json::to_string_pretty(v)?);
        }
        OutputFormat::Human => {
            for b in &v.blocks {
                eprintln!("block: [{}] {}", b.check, b.file.as_deref().unwrap_or(""));
                eprintln!("  {}", b.message);
            }
            for e in &v.errors {
                eprintln!(
                    "error: [{}] {} ({})",
                    e.check,
                    e.file.as_deref().unwrap_or(""),
                    e.reason
                );
                if let Some(d) = &e.detail {
                    eprintln!("  {d}");
                }
            }
            let line = match v.status {
                Status::Pass if no_match => "pass (no checks matched)",
                Status::Pass => "pass",
                Status::Block => "block",
                Status::InternalError => "internal_error",
                _ => "unknown",
            };
            println!("{line}");
        }
    }
    Ok(())
}

fn resolve_content_value(value: String) -> Result<String> {
    if value == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("failed to read --content from stdin (expected UTF-8)")?;
        Ok(buf)
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command name that cannot exist on PATH, so `shell_available` reports
    /// the unavailable path. Keeps the probe honest on a machine where `sh`
    /// does exist (macOS/Linux): we can't remove `sh`, but we can probe a name
    /// that is guaranteed not to resolve.
    const NO_SUCH_SHELL: &str = "ironlint-definitely-not-a-real-shell-xyz123";

    #[test]
    fn shell_available_false_for_nonexistent_command() {
        assert!(!shell_available(NO_SUCH_SHELL));
    }

    #[cfg(unix)]
    #[test]
    fn shell_available_true_for_sh_on_unix() {
        // On a Unix dev/CI machine `sh` is always present; this documents the
        // happy path and guards against a regression that breaks the probe.
        assert!(shell_available("sh"));
    }
}
