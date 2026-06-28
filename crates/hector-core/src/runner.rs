use crate::config::{Check, Config};
use crate::engine::{run_gate, GateEnv, GateOutcome};
use crate::telemetry::{LogEntry, PerCheckRecord};
use crate::verdict::{Block, GateError, Status, Verdict};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Optional per-run knobs for [`HectorEngine::check`]. Plumbed via
/// `builder().with_options(...)` so the public `check` signature stays stable
/// as knobs are added.
#[derive(Debug, Clone)]
pub struct CheckOptions {
    /// Restrict evaluation to these check ids. Empty set = run all checks. The
    /// filter is enforced before the check runs, so a filtered-out check never
    /// spawns a process.
    pub checks: HashSet<String>,
    /// What triggered this check, surfaced to checks as `$HECTOR_EVENT`.
    pub event: String,
    /// Allow checking files whose canonical path falls outside `config_dir`.
    /// Off by default so wrappers can't run policy against arbitrary host
    /// files.
    pub allow_external_paths: bool,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self {
            checks: HashSet::new(),
            event: "manual".to_string(),
            allow_external_paths: false,
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

/// Companion return shape for [`HectorEngine::check_with_explain`].
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

pub struct HectorEngine {
    config: Config,
    /// Directory containing the config file; check cwd + `$HECTOR_ROOT`.
    config_dir: PathBuf,
    /// Canonical form of `config_dir`, computed once at load time so
    /// `check_matches_path` doesn't `canonicalize()` the root on every call.
    config_dir_canon: PathBuf,
    options: CheckOptions,
    /// Per-check `ScopeMatcher` cache, keyed by check id and built once at load
    /// time so `check_matches_path` never rebuilds a GlobSet per (check, file).
    scope_matchers: BTreeMap<String, crate::config::scope::ScopeMatcher>,
    /// Per-check wall-clock budget (`HECTOR_TIMEOUT` env → `execution.timeout_secs`).
    timeout: Duration,
}

/// Per-check run result, before folding into the verdict. Skipped checks carry
/// a reason and contribute nothing to the verdict; ran checks carry their
/// wall-clock so telemetry can record it.
enum CheckStatus {
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
        elapsed: u64,
    },
}

/// Folded outcomes across every check for one file.
#[derive(Default)]
struct Collected {
    blocks: Vec<Block>,
    errors: Vec<GateError>,
    passed: Vec<String>,
    records: Vec<PerCheckRecord>,
    explain: Vec<CheckExplain>,
}

impl Collected {
    /// Fold one check's status into the running totals. `collect_explain`
    /// gates the per-check explain row; skipped checks contribute only an
    /// explain row (no verdict entry, no telemetry record).
    fn absorb(&mut self, check_id: &str, file: &str, status: CheckStatus, collect_explain: bool) {
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
                // Spec §3: a check that exits 2 with no output still needs a
                // human-readable message. The check layer (`classify`) has no
                // check id, so it returns ""; we fill it in here where the id
                // is known.
                let message = if message.is_empty() {
                    format!("{check_id} blocked")
                } else {
                    message
                };
                self.blocks.push(Block {
                    check: check_id.to_string(),
                    step: step.clone(),
                    file: Some(file.to_string()),
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
                elapsed,
            } => {
                self.errors.push(GateError {
                    check: check_id.to_string(),
                    step: step.clone(),
                    file: Some(file.to_string()),
                    reason: reason.clone(),
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

/// Resolve the per-check timeout: `HECTOR_TIMEOUT` (secs) overrides the config
/// value, which defaults to 30. Clamped to `>= 1` (a zero timeout would kill
/// every check instantly).
fn resolve_timeout(config: &Config) -> Duration {
    let secs = std::env::var("HECTOR_TIMEOUT")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(config.execution.timeout_secs);
    Duration::from_secs(secs.max(1))
}

/// Canonicalize `path` if it exists; otherwise walk up to the deepest existing
/// ancestor, canonicalize that, and re-append the missing tail. `None` only if
/// no ancestor exists.
///
/// Needed for PreToolUse `--content`: the proposed edit targets a path that may
/// not exist on disk yet. Plain `canonicalize` fails, but the parent typically
/// resolves, and macOS's `/var → /private/var` symlink means the parent's
/// canonical form differs from its literal form. Resolving through the parent
/// produces a path that `strip_prefix(config_dir_canon)` can match.
fn canonicalize_through_parent(path: &Path) -> Option<PathBuf> {
    if let Ok(c) = path.canonicalize() {
        return Some(c);
    }
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path.to_path_buf();
    while let Some(name) = cursor.file_name() {
        suffix.push(name.to_os_string());
        if !cursor.pop() {
            break;
        }
        if let Ok(c) = cursor.canonicalize() {
            let mut out = c;
            for seg in suffix.into_iter().rev() {
                out.push(seg);
            }
            return Some(out);
        }
    }
    None
}

/// Resolve `path` to a `config_dir`-relative form for scope matching, falling
/// back to the canonical absolute path when the input resolves outside the
/// config dir (bare-pattern globs still match absolute paths via their
/// `**/<pattern>` form).
fn relativize(path: &Path, root: &Path) -> PathBuf {
    let canon_path = canonicalize_through_parent(path).unwrap_or_else(|| PathBuf::from(path));
    let canon_root = root.canonicalize().unwrap_or_else(|_| PathBuf::from(root));
    canon_path
        .strip_prefix(&canon_root)
        .map(PathBuf::from)
        .unwrap_or(canon_path)
}

pub struct HectorEngineBuilder {
    options: CheckOptions,
}

impl HectorEngineBuilder {
    pub fn new() -> Self {
        Self {
            options: CheckOptions::default(),
        }
    }

    /// Attach optional per-run knobs (check filter, event, external paths).
    pub fn with_options(mut self, options: CheckOptions) -> Self {
        self.options = options;
        self
    }

    pub fn load(self, config_path: &Path) -> Result<HectorEngine> {
        HectorEngine::load_with(config_path, self.options)
    }
}

impl Default for HectorEngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl HectorEngine {
    pub fn load(config_path: &Path) -> Result<Self> {
        Self::load_with(config_path, CheckOptions::default())
    }

    pub fn builder() -> HectorEngineBuilder {
        HectorEngineBuilder::new()
    }

    /// Iterator over every check id in the loaded config. The CLI uses it to
    /// validate `--check` arguments at the boundary, before any dispatch.
    pub fn check_ids(&self) -> impl Iterator<Item = &str> {
        self.config.checks.keys().map(|k| k.as_str())
    }

    /// Replace the check-id filter on an already-loaded engine, so the CLI can
    /// load once, validate `--check` against the config, then store the
    /// validated set rather than loading twice.
    pub fn set_check_filter(&mut self, checks: HashSet<String>) {
        self.options.checks = checks;
    }

    fn load_with(config_path: &Path, options: CheckOptions) -> Result<Self> {
        // Debug hook: counts engine loads per process. Gated on the env var so
        // it is invisible in production; integration tests set it to assert
        // that `hector check` loads the engine exactly once.
        static LOAD_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = LOAD_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if std::env::var("HECTOR_DEBUG_LOAD_COUNT").is_ok() {
            eprintln!("hector_load_count={n}");
        }

        // Trust verification is gone in 0.3 (returns in a later plan as the
        // out-of-repo direnv store). `resolve` walks `extends:` without it.
        let config = crate::config::parse_file_with_extends(config_path)?;

        // Validate every check's file globs by constructing the matcher up
        // front, and cache it so `check_matches_path` never rebuilds a GlobSet.
        let mut scope_matchers: BTreeMap<String, crate::config::scope::ScopeMatcher> =
            BTreeMap::new();
        for (id, check) in &config.checks {
            let matcher = crate::config::scope::ScopeMatcher::new(&check.files)
                .with_context(|| format!("check `{id}` has invalid files glob"))?;
            scope_matchers.insert(id.clone(), matcher);
        }

        // Path::parent() returns Some("") for a bare relative filename
        // (e.g. ".hector.yml"), not None — filter that out so config_dir is
        // always a usable directory.
        let config_dir = config_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let config_dir_canon = config_dir
            .canonicalize()
            .unwrap_or_else(|_| config_dir.clone());
        let timeout = resolve_timeout(&config);

        Ok(Self {
            config,
            config_dir,
            config_dir_canon,
            options,
            scope_matchers,
            timeout,
        })
    }

    /// Resolve an input path argument against the engine's config dir.
    ///
    /// Absolute paths pass through; relative paths join onto `config_dir`. By
    /// default, returns `Err` when the canonicalized path falls outside
    /// `config_dir`; `allow_external_paths` opts in. Files that can't be
    /// canonicalized (e.g. pre-write paths not yet on disk) skip the
    /// outside-check and return the raw resolved path.
    pub fn resolve_input_path(&self, p: &Path) -> Result<PathBuf> {
        let resolved = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.config_dir.join(p)
        };
        let Ok(canon_input) = resolved.canonicalize() else {
            return Ok(resolved);
        };
        let canon_root = self
            .config_dir
            .canonicalize()
            .unwrap_or_else(|_| self.config_dir.clone());
        if !self.options.allow_external_paths && !canon_input.starts_with(&canon_root) {
            anyhow::bail!(
                "path {} resolves outside config_dir {}; pass --allow-external-paths to override",
                canon_input.display(),
                canon_root.display(),
            );
        }
        Ok(canon_input)
    }

    /// Match a path against a check's file globs using the load-time matcher
    /// cache. A relative path is matched directly (already config-dir-relative);
    /// an absolute path is stripped against the canonical config dir first.
    /// An unknown check id returns `false`.
    pub fn check_matches_path(&self, check_id: &str, file: &Path) -> bool {
        let match_path: PathBuf = if file.is_relative() {
            PathBuf::from(file)
        } else {
            let canon_file = file.canonicalize().unwrap_or_else(|_| PathBuf::from(file));
            canon_file
                .strip_prefix(&self.config_dir_canon)
                .map(PathBuf::from)
                .unwrap_or(canon_file)
        };
        self.scope_matchers
            .get(check_id)
            .map(|m| m.matches(&match_path))
            .unwrap_or(false)
    }

    /// The resolved (extends-merged) check map, in id order. Read-only; for
    /// `explain` / `show-resolved-config`.
    pub fn checks(&self) -> &std::collections::BTreeMap<String, crate::config::Check> {
        &self.config.checks
    }

    /// Why this check won't run for this file, or `None` if it should run.
    fn skip_reason(&self, check_id: &str, match_path: &Path, content: &str) -> Option<String> {
        if !self.options.checks.is_empty() && !self.options.checks.contains(check_id) {
            return Some("filtered".to_string());
        }
        if !self.check_matches_path(check_id, match_path) {
            return Some("out_of_scope".to_string());
        }
        if crate::disable::is_disabled(content, check_id) {
            return Some("disabled".to_string());
        }
        None
    }

    /// Run one check against one file: skip-check, then spawn and classify.
    fn run_one_check(
        &self,
        check_id: &str,
        check: &Check,
        abs: &Path,
        match_path: &Path,
        content: &str,
    ) -> CheckStatus {
        if let Some(reason) = self.skip_reason(check_id, match_path, content) {
            return CheckStatus::Skipped(reason);
        }
        let env = GateEnv {
            file: abs,
            root: &self.config_dir,
            event: &self.options.event,
        };
        let start = Instant::now();
        for step in check.effective_steps() {
            match run_gate(&step.run, &env, Some(content.as_bytes()), self.timeout) {
                GateOutcome::Pass => {}
                GateOutcome::Block { message } => {
                    let elapsed = start.elapsed().as_millis() as u64;
                    return CheckStatus::Block {
                        step: step.name.clone(),
                        message,
                        elapsed,
                    };
                }
                GateOutcome::Internal(reason) => {
                    let elapsed = start.elapsed().as_millis() as u64;
                    return CheckStatus::Error {
                        step: step.name.clone(),
                        reason: reason.as_str(),
                        elapsed,
                    };
                }
            }
        }
        let elapsed = start.elapsed().as_millis() as u64;
        CheckStatus::Pass(elapsed)
    }

    /// Run the loaded checks against `input` and return the verdict.
    pub fn check(&self, input: CheckInput) -> Result<Verdict> {
        self.check_inner(input, false).map(|r| r.verdict)
    }

    /// Like [`Self::check`], but always returns per-check explain rows. Cheap —
    /// the rows are a by-product of the same dispatch loop, no extra check runs.
    pub fn check_with_explain(&self, input: CheckInput) -> Result<CheckReport> {
        self.check_inner(input, true)
    }

    /// Central orchestration: resolve the path, run every check, fold the
    /// outcomes into a verdict, and log telemetry.
    fn check_inner(&self, input: CheckInput, collect_explain: bool) -> Result<CheckReport> {
        let start = Instant::now();
        let CheckInput::File { path, content } = input;
        let file_str = path.display().to_string();

        // An out-of-config_dir path is an argument/config error, not a check
        // outcome: propagate it as `Err` so the CLI maps it to exit 1. Folding
        // it into a synthetic `GateError` would yield exit 3 (InternalError),
        // which makes adapters fail OPEN — silently defeating the guard.
        let abs = self.resolve_input_path(&path)?;
        let match_path = relativize(&path, &self.config_dir);

        let mut collected = Collected::default();
        for (id, check) in &self.config.checks {
            let status = self.run_one_check(id, check, &abs, &match_path, &content);
            collected.absorb(id, &file_str, status, collect_explain);
        }

        let elapsed = start.elapsed().as_millis() as u64;
        let verdict = Verdict::from_outcomes(
            collected.blocks,
            collected.errors,
            collected.passed,
            elapsed,
        );
        self.append_check_log(
            &file_str,
            verdict.status,
            verdict.elapsed_ms,
            collected.records,
        );

        Ok(CheckReport {
            verdict,
            explain: collected.explain,
        })
    }

    /// Append one `Check` line to the telemetry log. Best-effort: a failed
    /// append warns to stderr but never fails the check — the log is never the
    /// source of truth.
    fn append_check_log(
        &self,
        file: &str,
        status: Status,
        elapsed_ms: u64,
        checks: Vec<PerCheckRecord>,
    ) {
        if let Err(e) = crate::telemetry::append(
            &self.config_dir.join(".hector/log.jsonl"),
            &LogEntry::Check {
                ts: chrono::Utc::now().to_rfc3339(),
                file: file.to_string(),
                event: self.options.event.clone(),
                status,
                elapsed_ms,
                checks,
            },
        ) {
            eprintln!("hector: telemetry append failed: {e:#}");
        }
    }
}

#[cfg(test)]
mod gate_dispatch_tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    #[test]
    fn matching_check_that_exits_2_blocks() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".hector.yml",
            "checks:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
        );
        let target = write(dir.path(), "a.rs", "// nothing\n");
        let engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
        let v = engine
            .check(CheckInput::File {
                path: target,
                content: "// TODO fix\n".into(),
            })
            .unwrap();
        assert_eq!(v.status, Status::Block);
        assert_eq!(v.blocks.len(), 1);
        assert_eq!(v.blocks[0].check, "no-todo");
    }

    #[test]
    fn non_matching_file_passes_with_no_checks_run() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".hector.yml",
            "checks:\n  ts-only:\n    files: \"**/*.ts\"\n    run: \"exit 2\"\n",
        );
        let target = write(dir.path(), "a.rs", "x\n");
        let engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
        let v = engine
            .check(CheckInput::File {
                path: target,
                content: "x\n".into(),
            })
            .unwrap();
        assert_eq!(v.status, Status::Pass);
        assert!(v.passed.is_empty());
    }

    #[test]
    fn broken_check_is_internal_error() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".hector.yml",
            "checks:\n  oops:\n    files: \"**/*.rs\"\n    run: \"definitely-not-real-xyz\"\n",
        );
        let target = write(dir.path(), "a.rs", "x\n");
        let engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
        let v = engine
            .check(CheckInput::File {
                path: target,
                content: "x\n".into(),
            })
            .unwrap();
        assert_eq!(v.status, Status::InternalError);
        assert_eq!(v.errors[0].reason, "not_found");
    }

    #[test]
    fn block_with_no_output_uses_check_id_message() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".hector.yml",
            "checks:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n",
        );
        let target = write(dir.path(), "a.rs", "x\n");
        let engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
        let v = engine
            .check(CheckInput::File {
                path: target,
                content: "x\n".into(),
            })
            .unwrap();
        assert_eq!(v.status, Status::Block);
        assert_eq!(v.blocks[0].message, "no-todo blocked");
    }

    #[test]
    fn explain_reports_per_check_outcome() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".hector.yml"),
            "checks:\n  blocker:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n  passer:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
        )
        .unwrap();
        let target = dir.path().join("a.rs");
        std::fs::write(&target, "x\n").unwrap();
        let engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
        let report = engine
            .check_with_explain(CheckInput::File {
                path: target,
                content: "x\n".into(),
            })
            .unwrap();
        let outcomes: std::collections::HashMap<_, _> = report
            .explain
            .iter()
            .map(|r| {
                (
                    r.check_id.clone(),
                    matches!(r.outcome, ExplainOutcome::Fire),
                )
            })
            .collect();
        assert!(outcomes["blocker"]);
        assert!(!outcomes["passer"]);
    }

    #[test]
    fn check_filter_skips_unselected_checks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".hector.yml"),
            "checks:\n  blocker:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n  other:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n",
        )
        .unwrap();
        let target = dir.path().join("a.rs");
        std::fs::write(&target, "x\n").unwrap();
        let mut engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
        engine.set_check_filter(std::iter::once("other".to_string()).collect());
        let v = engine
            .check(CheckInput::File {
                path: target,
                content: "x\n".into(),
            })
            .unwrap();
        assert_eq!(v.blocks.len(), 1);
        assert_eq!(v.blocks[0].check, "other");
    }

    #[test]
    fn checks_accessor_returns_loaded_check_ids() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".hector.yml",
            "checks:\n  alpha:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n  beta:\n    files: \"**/*.ts\"\n    run: \"exit 0\"\n",
        );
        let engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
        let ids: Vec<&str> = engine.checks().keys().map(|k| k.as_str()).collect();
        // BTreeMap iterates in key order
        assert_eq!(ids, vec!["alpha", "beta"]);
    }

    #[test]
    fn hector_file_is_absolute_for_checks() {
        // ABI lock: `$HECTOR_FILE` handed to a check is always an absolute path,
        // so a check can match it without guessing whether it's relative. The
        // check blocks (exit 2) iff `$HECTOR_FILE` is *not* absolute; a Pass
        // verdict proves the engine resolved it to an absolute path. Guards the
        // pi-harness report that `$HECTOR_FILE` was unexpectedly relative.
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".hector.yml",
            "checks:\n  abs:\n    files: \"**/*.rs\"\n    run: \"case \\\"$HECTOR_FILE\\\" in /*) exit 0;; *) exit 2;; esac\"\n",
        );
        let target = write(dir.path(), "a.rs", "x\n");
        let engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
        let v = engine
            .check(CheckInput::File {
                path: target,
                content: "x\n".into(),
            })
            .unwrap();
        assert_eq!(
            v.status,
            Status::Pass,
            "$HECTOR_FILE must be absolute (check blocks on a non-absolute path): {:?}",
            v.blocks
        );
    }

    #[test]
    fn disable_directive_suppresses_a_check() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".hector.yml"),
            "checks:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n",
        )
        .unwrap();
        let target = dir.path().join("a.rs");
        std::fs::write(&target, "x\n").unwrap();
        let engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
        let v = engine
            .check(CheckInput::File {
                path: target,
                content: "// hector-disable: no-todo\n".into(),
            })
            .unwrap();
        assert_eq!(v.status, Status::Pass);
        assert!(v.blocks.is_empty());
    }

    // --- Phase 2: steps fail-fast ---

    #[test]
    fn steps_fail_fast_on_first_blocking_step() {
        // step 1 passes (exit 0), step 2 blocks (exit 2),
        // step 3 must NOT run. Use a sentinel file to prove step 3 was skipped.
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".hector.yml",
            "checks:\n  g:\n    files: \"*\"\n    steps:\n      - run: \"true\"\n      - name: blocker\n        run: \"echo nope; exit 2\"\n      - run: \"touch ran3.txt\"\n",
        );
        let target = write(dir.path(), "x.txt", "body");
        let engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
        let v = engine
            .check(CheckInput::File {
                path: target,
                content: "body".into(),
            })
            .unwrap();
        assert_eq!(v.status, Status::Block);
        assert_eq!(v.blocks[0].step.as_deref(), Some("blocker"));
        assert!(
            !dir.path().join("ran3.txt").exists(),
            "step 3 ran after a block"
        );
    }
}
