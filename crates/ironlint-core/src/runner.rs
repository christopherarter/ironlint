use crate::config::{Check, Config};
use crate::engine::{run_gate, GateEnv, GateOutcome, InternalReason};
use crate::telemetry::{LogEntry, PerCheckRecord};
use crate::verdict::{Block, GateError, Status, Verdict};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

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

pub struct IronLintEngine {
    config: Config,
    /// Directory containing the config file; check cwd + `$IRONLINT_ROOT`.
    config_dir: PathBuf,
    /// Canonical form of `config_dir`, computed once at load time so
    /// `check_matches_path` doesn't `canonicalize()` the root on every call.
    config_dir_canon: PathBuf,
    options: CheckOptions,
    /// Per-check `ScopeMatcher` cache, keyed by check id and built once at load
    /// time so `check_matches_path` never rebuilds a GlobSet per (check, file).
    scope_matchers: BTreeMap<String, crate::config::scope::ScopeMatcher>,
    /// Per-check wall-clock budget (`IRONLINT_TIMEOUT` env → `execution.timeout_secs`).
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
        detail: Option<String>,
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
    ///
    /// `file` is `None` for set-level (pre-commit) invocations — the
    /// resulting `Block.file` / `GateError.file` will be `null` in the JSON.
    fn absorb(
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

/// Resolve the per-check timeout: `IRONLINT_TIMEOUT` (secs) overrides the config
/// value, which defaults to 30. An ambient override below
/// [`IRONLINT_TIMEOUT_FLOOR_SECS`] is raised to the floor and a loud warning is
/// emitted, so a prompt-injected short timeout cannot force every check to time
/// out (-> exit 3 -> fail-open). The base `>= 1` clamp (see
/// [`resolve_timeout_secs`]) still applies.
fn resolve_timeout(config: &Config) -> Duration {
    let env_val = std::env::var("IRONLINT_TIMEOUT").ok();
    let (secs, shortened) = resolve_timeout_with_floor(env_val.as_deref(), config.timeout_secs());
    if shortened {
        eprintln!(
            "ironlint: IRONLINT_TIMEOUT={} is below the {}s floor (config timeout_secs={}); \
             raising to {}s so a real check is not forced to time out. Set execution.timeout_secs \
             in .ironlint.yml to silence this for a trusted, shorter budget.",
            env_val.as_deref().unwrap_or(""),
            IRONLINT_TIMEOUT_FLOOR_SECS,
            config.timeout_secs(),
            IRONLINT_TIMEOUT_FLOOR_SECS,
        );
    }
    Duration::from_secs(secs)
}

/// Minimum seconds an ambient `IRONLINT_TIMEOUT` override is allowed to
/// impose. A maliciously short ambient value (e.g. `IRONLINT_TIMEOUT=1`
/// from a prompt-injected agent or a repo `.envrc`) would force every real
/// check to time out -> exit 3 -> fail-open; flooring it at this value keeps
/// real checks runnable. Applies ONLY to ambient overrides — an explicit
/// `execution.timeout_secs: N` in the (trusted) config is the operator's
/// choice and is not raised.
const IRONLINT_TIMEOUT_FLOOR_SECS: u64 = 10;

/// Pure resolver behind [`resolve_timeout`]: `env_val` (the raw
/// `IRONLINT_TIMEOUT` string, if any) wins when it parses as a `u64`;
/// otherwise `config_default` is used. The result is always clamped to
/// `>= 1`, regardless of which source it came from. Extracted as a pure
/// function (no env access) so the override + clamp behavior is unit-testable
/// without mutating process-global env state.
fn resolve_timeout_secs(env_val: Option<&str>, config_default: u64) -> u64 {
    env_val
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(config_default)
        .max(1)
}

/// Resolve the timeout with the ambient-override floor applied. Pure (no env
/// access) so the floor behavior is unit-testable without mutating
/// process-global env. Returns `(secs, shortened)`:
/// - `secs`: when `env_val` parses as a `u64`, the clamped seconds (via
///   [`resolve_timeout_secs`], which keeps its own `>= 1` floor) raised to
///   [`IRONLINT_TIMEOUT_FLOOR_SECS`] if below it; otherwise the parsed value.
///   When `env_val` is `None` or unparseable, `config_default` is used as-is
///   (still subject to `resolve_timeout_secs`' `>= 1` clamp) and NOT floored.
/// - `shortened`: `true` iff an ambient override was present, parseable, and
///   below the floor — the one case the caller should log loudly. Surfaced
///   as a return value so the log decision is testable without stderr capture.
fn resolve_timeout_with_floor(env_val: Option<&str>, config_default: u64) -> (u64, bool) {
    let secs = resolve_timeout_secs(env_val, config_default);
    let shortened = matches!(
        env_val.and_then(|s| s.parse::<u64>().ok()),
        Some(parsed) if parsed < IRONLINT_TIMEOUT_FLOOR_SECS
    );
    let secs = if shortened {
        secs.max(IRONLINT_TIMEOUT_FLOOR_SECS)
    } else {
        secs
    };
    (secs, shortened)
}

#[cfg(test)]
mod resolve_timeout_secs_tests {
    use super::{resolve_timeout_secs, resolve_timeout_with_floor, IRONLINT_TIMEOUT_FLOOR_SECS};

    #[test]
    fn valid_env_override_wins_over_config_default() {
        assert_eq!(resolve_timeout_secs(Some("5"), 30), 5);
    }

    #[test]
    fn zero_env_override_is_clamped_to_one() {
        assert_eq!(resolve_timeout_secs(Some("0"), 30), 1);
    }

    #[test]
    fn unparseable_env_override_falls_back_to_config_default() {
        assert_eq!(resolve_timeout_secs(Some("notanumber"), 30), 30);
    }

    #[test]
    fn unset_env_override_uses_config_default() {
        assert_eq!(resolve_timeout_secs(None, 30), 30);
    }

    #[test]
    fn config_default_itself_is_clamped_to_one() {
        // A configured `timeout_secs: 0` must also be clamped — the clamp
        // applies to whichever source (env or config) supplied the value.
        assert_eq!(resolve_timeout_secs(None, 0), 1);
    }

    #[test]
    fn floor_raises_short_ambient_override_to_minimum() {
        // An ambient IRONLINT_TIMEOUT=1 (the prompt-injection attack) is
        // floored up to IRONLINT_TIMEOUT_FLOOR_SECS so a real check is not
        // forced to time out -> exit 3 -> fail-open. `shortened` flags the
        // log should fire.
        let (secs, shortened) = resolve_timeout_with_floor(Some("1"), 30);
        assert_eq!(secs, IRONLINT_TIMEOUT_FLOOR_SECS);
        assert!(shortened, "a shortened ambient override must flag the log");
    }

    #[test]
    fn floor_passes_through_large_ambient_override() {
        // A legitimate ambient override at or above the floor passes through
        // unchanged — the floor is a minimum, not a cap — and does not flag.
        let (secs, shortened) = resolve_timeout_with_floor(Some("60"), 30);
        assert_eq!(secs, 60);
        assert!(!shortened);
    }

    #[test]
    fn floor_does_not_touch_the_config_default_path() {
        // No ambient override => the operator's config_default is used as-is
        // (still subject to the .max(1) floor inside resolve_timeout_secs,
        // but NOT raised to IRONLINT_TIMEOUT_FLOOR_SECS — that would silently
        // widen an explicit, trusted operator choice) and does not flag.
        let (secs, shortened) = resolve_timeout_with_floor(None, 5);
        assert_eq!(secs, 5);
        assert!(!shortened);
    }
}

/// Map the current event string to a [`Lifecycle`] variant for the `on:` filter.
///
/// The CLI value_parser guarantees only `write` and `pre-commit` reach the
/// engine; everything else (e.g. `manual` in tests) maps to `Write` so
/// existing tests that don't set an event still run write-subscribed checks.
fn event_lifecycle(event: &str) -> crate::config::Lifecycle {
    match event {
        "pre-commit" => crate::config::Lifecycle::PreCommit,
        _ => crate::config::Lifecycle::Write,
    }
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

/// Removes its temp file on drop — covers normal return, error, timeout, and
/// panic-unwind. `Drop` does **not** run across a hard process kill: a
/// `SIGTERM`/`SIGINT`/`SIGKILL` that ends ironlint mid-check (harnesses
/// routinely do this on their own hook timeout budget, not just on a manual
/// `kill -9`) skips this destructor entirely and leaves the file sitting in
/// the checked file's own directory — which for real source is nested (e.g.
/// `crates/foo/src/`), not the config root. Two `sweep_stale_tmpfiles`
/// backstops reclaim that leak once it's older than its age threshold:
/// `maybe_materialize_tmpfile` sweeps the tmpfile's own parent directory
/// immediately before writing a new tmpfile there, so a later check against
/// any file in that same directory reclaims the leak; the sweep at engine
/// `load()` additionally covers leaks sitting directly in the config root.
struct TmpFileGuard {
    path: PathBuf,
}

impl Drop for TmpFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// True iff any of the check's steps reference the `$IRONLINT_TMPFILE` token.
fn check_references_tmpfile(check: &Check) -> bool {
    check
        .effective_steps()
        .iter()
        .any(|s| s.run.contains("IRONLINT_TMPFILE"))
}

/// The naming prefix ironlint gives every `$IRONLINT_TMPFILE` it
/// materializes (see `unique_tmp_name`) — the only pattern
/// `sweep_stale_tmpfiles` is allowed to remove.
const TMPFILE_PREFIX: &str = "ironlint-tmp-";

/// A collision-resistant temp-file name mirroring `ext` (no `rng` dependency).
fn unique_tmp_name(ext: Option<&str>) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::UNIX_EPOCH;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    match ext {
        Some(e) => format!("{TMPFILE_PREFIX}{pid}-{n}-{nanos}.{e}"),
        None => format!("{TMPFILE_PREFIX}{pid}-{n}-{nanos}"),
    }
}

/// Default age threshold for `sweep_stale_tmpfiles`, used both at engine
/// `load()` (config-root sweep) and at tmpfile materialization time (sweep
/// of the tmpfile's own parent directory — see `maybe_materialize_tmpfile`).
/// A tmpfile older than this is assumed to be a leak from a killed prior run
/// rather than one still in flight: an in-progress check's own wall-clock
/// cap is `execution.timeout_secs` (default 30s, see `resolve_timeout`), so
/// an hour of headroom keeps a live tmpfile — even from a slow,
/// concurrently-running ironlint process — comfortably below the threshold
/// and never a sweep target. This is what makes it safe to run the sweep
/// unconditionally rather than coordinating with other in-flight processes:
/// age, not identity, is the only signal, and a generous threshold keeps
/// false positives effectively impossible. That safety margin holds only
/// while the configured per-check timeout (`execution.timeout_secs`) stays
/// well under this 1h threshold — a check configured with an hours-long
/// timeout would put its own still-live tmpfile at risk of being swept as a
/// leak.
const TMPFILE_SWEEP_MAX_AGE: Duration = Duration::from_hours(1);

/// Best-effort reclaim of `$IRONLINT_TMPFILE` leaks in `root`'s immediate
/// directory: removes only files whose name starts with `TMPFILE_PREFIX` and
/// whose mtime is older than `max_age`. Never touches anything else — a
/// non-matching name, a directory, or an unreadable/racing entry is simply
/// skipped rather than erroring, since this runs unconditionally on every
/// engine load and a sweep failure must never block a check.
///
/// Deliberately shallow (does not recurse into subdirectories): a tmpfile
/// for a nested checked file lands in that file's own directory (see
/// `maybe_materialize_tmpfile`), which this pass does not visit. `load()` is
/// a hot path — it can run once per agent write — so the sweep is bounded to
/// `root`'s own entries rather than paying an O(repo size) walk on every
/// invocation.
fn sweep_stale_tmpfiles(root: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        if is_stale_tmpfile(&entry, now, max_age) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// True iff `entry` is a regular file matching `TMPFILE_PREFIX` whose mtime
/// is older than `max_age` relative to `now`. Extracted from
/// `sweep_stale_tmpfiles` to keep both functions well under the cognitive
/// complexity cap.
fn is_stale_tmpfile(entry: &std::fs::DirEntry, now: SystemTime, max_age: Duration) -> bool {
    let file_name = entry.file_name();
    let Some(name) = file_name.to_str() else {
        return false;
    };
    if !name.starts_with(TMPFILE_PREFIX) {
        return false;
    }
    let Ok(metadata) = entry.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    // `Err` means `modified` is in the future relative to `now` (clock skew
    // or a filesystem with coarse/odd mtime semantics) — treat as fresh, not
    // stale, rather than guessing.
    now.duration_since(modified)
        .map(|age| age > max_age)
        .unwrap_or(false)
}

pub struct IronLintEngineBuilder {
    options: CheckOptions,
}

impl IronLintEngineBuilder {
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

    pub fn load(self, config_path: &Path) -> Result<IronLintEngine> {
        IronLintEngine::load_with(config_path, self.options)
    }
}

impl Default for IronLintEngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Create the `$IRONLINT_TMPFILE` at `path` with `content`, using an
/// exclusive (`O_EXCL` / `create_new`) open at mode `0600` on unix.
///
/// Exclusive create makes a symlink race fail-closed: if `path` already
/// exists (attacker pre-creates it), the open errors instead of clobbering.
/// Mode `0600` keeps the briefly-written proposed content unreadable by
/// co-located processes (the old `std::fs::write` left it world-readable
/// ~`0o644`). On non-unix the mode bit is unavailable; fall back to a plain
/// exclusive create + write (the name is still collision-resistant).
fn materialize_tmpfile(path: &Path, content: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(content.as_bytes())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(content.as_bytes())?;
        Ok(())
    }
}

impl IronLintEngine {
    pub fn load(config_path: &Path) -> Result<Self> {
        Self::load_with(config_path, CheckOptions::default())
    }

    pub fn builder() -> IronLintEngineBuilder {
        IronLintEngineBuilder::new()
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

    /// The event string stored in this engine's options. Used by the CLI to
    /// pick the right dispatch path (`check_set` vs per-file loop) in `run_diff`.
    pub fn event(&self) -> &str {
        &self.options.event
    }

    fn load_with(config_path: &Path, options: CheckOptions) -> Result<Self> {
        // Debug hook: counts engine loads per process. Gated on the env var so
        // it is invisible in production; integration tests set it to assert
        // that `ironlint check` loads the engine exactly once.
        static LOAD_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = LOAD_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if std::env::var("IRONLINT_DEBUG_LOAD_COUNT").is_ok() {
            eprintln!("ironlint_load_count={n}");
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
        // (e.g. ".ironlint.yml"), not None — filter that out so config_dir is
        // always a usable directory.
        let config_dir = config_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let config_dir_canon = config_dir
            .canonicalize()
            .unwrap_or_else(|_| config_dir.clone());

        // Reclaim any $IRONLINT_TMPFILE a prior run leaked by dying mid-check
        // (SIGTERM/SIGINT skip TmpFileGuard::drop — see its docstring).
        // Best-effort and age-gated, so it can never step on a tmpfile a
        // concurrently-running ironlint process still owns.
        sweep_stale_tmpfiles(&config_dir_canon, TMPFILE_SWEEP_MAX_AGE);

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

    /// Make a path absolute relative to the project root for ABI env vars.
    /// Absolute paths pass through; relative paths join onto `config_dir`.
    fn absolutize_for_env(&self, p: &Path) -> PathBuf {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.config_dir.join(p)
        }
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
    ///
    /// Checks in order: check-id filter → scope → disable directive → `on:` lifecycle.
    fn skip_reason(
        &self,
        check_id: &str,
        check: &Check,
        match_path: &Path,
        content: &str,
    ) -> Option<String> {
        if !self.options.checks.is_empty() && !self.options.checks.contains(check_id) {
            return Some("filtered".to_string());
        }
        if !self.check_matches_path(check_id, match_path) {
            let forced = self.options.force && self.options.checks.contains(check_id);
            if !forced {
                return Some("out_of_scope".to_string());
            }
        }
        if crate::disable::is_disabled(content, check_id) {
            return Some("disabled".to_string());
        }
        if !check.on.contains(&event_lifecycle(&self.options.event)) {
            return Some("event".to_string());
        }
        None
    }

    /// Materialize `$IRONLINT_TMPFILE` for a `write` check that references it.
    /// `Ok(None)` = not needed; `Ok(Some)` = created (guard owns cleanup);
    /// `Err` = the write failed (caller surfaces it as an internal error so a
    /// tmpfile-dependent check never silently runs without its file).
    fn maybe_materialize_tmpfile(
        &self,
        check: &Check,
        abs: &Path,
        content: &str,
    ) -> std::io::Result<Option<TmpFileGuard>> {
        if self.options.event != "write" || !check_references_tmpfile(check) {
            return Ok(None);
        }
        let Some(parent) = abs.parent() else {
            return Ok(None);
        };
        // Containment: never write proposed content outside the project root
        // unless the caller opted into external paths. `resolve_input_path`'s
        // guard is bypassed for not-yet-canonicalizable (new-file) paths — the
        // common pre-write case — so re-check the parent here before writing.
        if !self.options.allow_external_paths {
            let parent_canon = parent.canonicalize()?;
            if !parent_canon.starts_with(&self.config_dir_canon) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "refusing to materialize $IRONLINT_TMPFILE outside the project root: {}",
                        parent_canon.display()
                    ),
                ));
            }
        }
        // Reclaim any stale ironlint-tmp-* leak sitting in THIS directory
        // before adding a new one. This is the leak's actual home (see the
        // module doc on `maybe_materialize_tmpfile` / `TmpFileGuard`), so
        // sweeping here — not just at load-time over the config root — is
        // what makes a nested leak (the common case for real source) ever
        // get reclaimed. One `read_dir` of `parent`, only for write-event
        // checks that reference `$IRONLINT_TMPFILE`; the file this call is
        // about to create doesn't exist yet, so it can never sweep itself.
        sweep_stale_tmpfiles(parent, TMPFILE_SWEEP_MAX_AGE);

        let name = unique_tmp_name(abs.extension().and_then(|e| e.to_str()));
        let path = parent.join(name);
        materialize_tmpfile(&path, content)?;
        Ok(Some(TmpFileGuard { path }))
    }

    /// Build the human-readable `GateError.detail` string for an internal error:
    /// names the (truncated) run command and, for timeouts, the effective
    /// timeout that fired. One line.
    fn detail_for(reason: &InternalReason, run: &str, timeout: Duration) -> String {
        const MAX_RUN_LEN: usize = 80;
        let run_trunc = if run.len() > MAX_RUN_LEN {
            format!("{}…", &run[..MAX_RUN_LEN])
        } else {
            run.to_string()
        };
        match reason {
            InternalReason::Timeout => {
                format!(
                    "timeout after {}s running: {}",
                    timeout.as_secs(),
                    run_trunc
                )
            }
            _ => format!("{} running: {}", reason.as_str(), run_trunc),
        }
    }

    /// Execute a check's step pipeline against `env` with optional `content` on
    /// stdin. Fails fast on the first Block or Internal — never panics.
    fn run_steps(&self, check: &Check, env: &GateEnv, content: Option<&[u8]>) -> CheckStatus {
        let start = Instant::now();
        for step in check.effective_steps() {
            match run_gate(&step.run, env, content, self.timeout) {
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
                        detail: Some(Self::detail_for(&reason, &step.run, self.timeout)),
                        elapsed,
                    };
                }
            }
        }
        let elapsed = start.elapsed().as_millis() as u64;
        CheckStatus::Pass(elapsed)
    }

    /// Run one check against one file: skip-check, build env, then run steps.
    fn run_one_check(
        &self,
        check_id: &str,
        check: &Check,
        abs: &Path,
        match_path: &Path,
        content: &str,
    ) -> CheckStatus {
        if let Some(reason) = self.skip_reason(check_id, check, match_path, content) {
            return CheckStatus::Skipped(reason);
        }
        let tmp = match self.maybe_materialize_tmpfile(check, abs, content) {
            Ok(t) => t,
            Err(e) => {
                return CheckStatus::Error {
                    step: Some("<tmpfile>".to_string()),
                    reason: format!("tmpfile_write_failed:{e}"),
                    detail: None,
                    elapsed: 0,
                }
            }
        };
        let abs_buf = abs.to_path_buf();
        let env = GateEnv {
            file: Some(abs),
            files: std::slice::from_ref(&abs_buf),
            root: &self.config_dir,
            event: &self.options.event,
            tmpfile: tmp.as_ref().map(|g| g.path.as_path()),
        };
        self.run_steps(check, &env, Some(content.as_bytes()))
        // `tmp` drops here → temp file removed.
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

    /// Pre-commit (run-once) dispatch: run each check **once** over the subset
    /// of `files` that match its scope, with `$IRONLINT_FILES` and empty stdin.
    ///
    /// Checks whose `on:` list excludes the current event are skipped. Checks
    /// with no matching files are skipped. The resulting `Block.file` /
    /// `GateError.file` are `null` because there is no single primary target.
    ///
    /// # Disable directives
    ///
    /// Inline `ironlint-disable: <id>` directives are a write-lifecycle,
    /// per-file feature — they are scanned from the proposed file content
    /// supplied at write time. In pre-commit/set mode there is no per-file
    /// content, so inline disable directives are NOT evaluated here.
    pub fn check_set(&self, files: &[PathBuf]) -> Result<Verdict> {
        let start = Instant::now();
        let mut collected = Collected::default();

        for (check_id, check) in &self.config.checks {
            // Check-id filter (same gate as per-file mode).
            if !self.options.checks.is_empty() && !self.options.checks.contains(check_id) {
                continue;
            }
            // Lifecycle filter: only run checks subscribed to the current event.
            if !check.on.contains(&event_lifecycle(&self.options.event)) {
                continue;
            }
            // Scope: compute the subset of `files` this check cares about.
            let matched: Vec<PathBuf> = files
                .iter()
                .filter(|f| self.check_matches_path(check_id, f))
                .map(|f| self.absolutize_for_env(f))
                .collect();
            if matched.is_empty() {
                continue;
            }
            // Run the check once over the matched set; stdin is closed (None).
            let env = GateEnv {
                file: None,
                files: &matched,
                root: &self.config_dir,
                event: &self.options.event,
                tmpfile: None,
            };
            let status = self.run_steps(check, &env, None);
            collected.absorb(check_id, None, status, false);
        }

        let elapsed = start.elapsed().as_millis() as u64;
        let verdict = Verdict::from_outcomes(
            collected.blocks,
            collected.errors,
            collected.passed,
            elapsed,
        );
        // Log telemetry as a set-level invocation; `file` is absent (no single
        // target), `set_size` records how many files were in the checked set.
        self.append_check_log(
            None,
            Some(files.len()),
            verdict.status,
            verdict.elapsed_ms,
            collected.records,
        );

        Ok(verdict)
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
            collected.absorb(id, Some(&file_str), status, collect_explain);
        }

        let elapsed = start.elapsed().as_millis() as u64;
        let verdict = Verdict::from_outcomes(
            collected.blocks,
            collected.errors,
            collected.passed,
            elapsed,
        );
        self.append_check_log(
            Some(&file_str),
            None,
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
    ///
    /// `file` is `Some` for write-lifecycle (per-file) invocations and `None`
    /// for pre-commit/set invocations. `set_size` is the inverse: `Some(n)` on
    /// pre-commit with the number of files in the checked set, `None` on
    /// per-file records.
    fn append_check_log(
        &self,
        file: Option<&str>,
        set_size: Option<usize>,
        status: Status,
        elapsed_ms: u64,
        checks: Vec<PerCheckRecord>,
    ) {
        if let Err(e) = crate::telemetry::append(
            &self.config_dir.join(".ironlint/log.jsonl"),
            &LogEntry::Check {
                ts: chrono::Utc::now().to_rfc3339(),
                file: file.map(|f| f.to_string()),
                set_size,
                event: self.options.event.clone(),
                status,
                elapsed_ms,
                checks,
            },
        ) {
            eprintln!("ironlint: telemetry append failed: {e:#}");
        }
    }
}

#[cfg(test)]
mod gate_dispatch_tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    // --- Phase 4 test helpers ---

    fn write_config(dir: &TempDir, body: &str) {
        std::fs::write(dir.path().join(".ironlint.yml"), body).unwrap();
    }

    fn load_with_event(dir: &TempDir, event: &str) -> IronLintEngine {
        IronLintEngine::builder()
            .with_options(CheckOptions {
                event: event.to_string(),
                ..Default::default()
            })
            .load(&dir.path().join(".ironlint.yml"))
            .unwrap()
    }

    fn file_input(dir: &TempDir, name: &str, content: &str) -> CheckInput {
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        CheckInput::File {
            path,
            content: content.to_string(),
        }
    }

    fn touch(dir: &TempDir, name: &str) {
        std::fs::write(dir.path().join(name), "").unwrap();
    }

    fn abs(dir: &TempDir, name: &str) -> PathBuf {
        dir.path().join(name)
    }

    #[test]
    fn matching_check_that_exits_2_blocks() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".ironlint.yml",
            "checks:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
        );
        let target = write(dir.path(), "a.rs", "// nothing\n");
        let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
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
            ".ironlint.yml",
            "checks:\n  ts-only:\n    files: \"**/*.ts\"\n    run: \"exit 2\"\n",
        );
        let target = write(dir.path(), "a.rs", "x\n");
        let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
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
            ".ironlint.yml",
            "checks:\n  oops:\n    files: \"**/*.rs\"\n    run: \"definitely-not-real-xyz\"\n",
        );
        let target = write(dir.path(), "a.rs", "x\n");
        let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
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
        // Unnamed step (plain `run:`) → "<check-id> blocked"
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".ironlint.yml",
            "checks:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n",
        );
        let target = write(dir.path(), "a.rs", "x\n");
        let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
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
    fn block_with_no_output_and_named_step_uses_step_name_in_message() {
        // Named blocking step → "<check-id> › <step-name> blocked" (spec §5)
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".ironlint.yml",
            "checks:\n  ts-quality:\n    files: \"**/*.ts\"\n    steps:\n      - name: no-any\n        run: \"exit 2\"\n",
        );
        let target = write(dir.path(), "a.ts", "x\n");
        let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
        let v = engine
            .check(CheckInput::File {
                path: target,
                content: "x\n".into(),
            })
            .unwrap();
        assert_eq!(v.status, Status::Block);
        assert_eq!(v.blocks[0].message, "ts-quality \u{203a} no-any blocked");
    }

    #[test]
    fn explain_reports_per_check_outcome() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".ironlint.yml"),
            "checks:\n  blocker:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n  passer:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
        )
        .unwrap();
        let target = dir.path().join("a.rs");
        std::fs::write(&target, "x\n").unwrap();
        let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
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
            dir.path().join(".ironlint.yml"),
            "checks:\n  blocker:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n  other:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n",
        )
        .unwrap();
        let target = dir.path().join("a.rs");
        std::fs::write(&target, "x\n").unwrap();
        let mut engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
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
            ".ironlint.yml",
            "checks:\n  alpha:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n  beta:\n    files: \"**/*.ts\"\n    run: \"exit 0\"\n",
        );
        let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
        let ids: Vec<&str> = engine.checks().keys().map(|k| k.as_str()).collect();
        // BTreeMap iterates in key order
        assert_eq!(ids, vec!["alpha", "beta"]);
    }

    #[test]
    fn ironlint_file_is_absolute_for_checks() {
        // ABI lock: `$IRONLINT_FILE` handed to a check is always an absolute path,
        // so a check can match it without guessing whether it's relative. The
        // check blocks (exit 2) iff `$IRONLINT_FILE` is *not* absolute; a Pass
        // verdict proves the engine resolved it to an absolute path. Guards the
        // pi-harness report that `$IRONLINT_FILE` was unexpectedly relative.
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".ironlint.yml",
            "checks:\n  abs:\n    files: \"**/*.rs\"\n    run: \"case \\\"$IRONLINT_FILE\\\" in /*) exit 0;; *) exit 2;; esac\"\n",
        );
        let target = write(dir.path(), "a.rs", "x\n");
        let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
        let v = engine
            .check(CheckInput::File {
                path: target,
                content: "x\n".into(),
            })
            .unwrap();
        assert_eq!(
            v.status,
            Status::Pass,
            "$IRONLINT_FILE must be absolute (check blocks on a non-absolute path): {:?}",
            v.blocks
        );
    }

    #[test]
    fn ironlint_files_are_absolute_for_pre_commit_set() {
        // ABI lock: `$IRONLINT_FILES` handed to a pre-commit check is always
        // newline-joined absolute paths. The check blocks (exit 2) iff any
        // entry in `$IRONLINT_FILES` is not absolute.
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".ironlint.yml",
            "checks:\n  abs:\n    files: \"**/*.rs\"\n    on: [pre-commit]\n    run: \"for p in \\\"$IRONLINT_FILES\\\"; do case \\\"$p\\\" in /*) ;; *) exit 2;; esac; done\"\n",
        );
        touch(&dir, "a.rs");
        touch(&dir, "b.rs");
        let engine = load_with_event(&dir, "pre-commit");
        // Pass RELATIVE paths into check_set, simulating the CLI --diff path.
        let v = engine
            .check_set(&[PathBuf::from("a.rs"), PathBuf::from("b.rs")])
            .unwrap();
        assert_eq!(
            v.status,
            Status::Pass,
            "$IRONLINT_FILES entries must be absolute (check blocks on a non-absolute path): {:?}",
            v.blocks
        );
    }

    #[test]
    fn disable_directive_suppresses_a_check() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".ironlint.yml"),
            "checks:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n",
        )
        .unwrap();
        let target = dir.path().join("a.rs");
        std::fs::write(&target, "x\n").unwrap();
        let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
        let v = engine
            .check(CheckInput::File {
                path: target,
                content: "// ironlint-disable: no-todo\n".into(),
            })
            .unwrap();
        assert_eq!(v.status, Status::Pass);
        assert!(v.blocks.is_empty());
    }

    // --- Phase 4: on: filter + pre-commit run-once ---

    #[test]
    fn on_filter_skips_write_only_check_at_pre_commit() {
        let dir = tempfile::tempdir().unwrap();
        write_config(
            &dir,
            "checks:\n  g:\n    files: \"*\"\n    run: \"exit 2\"\n",
        ); // on defaults to [write]
        let engine = load_with_event(&dir, "pre-commit");
        let v = engine.check(file_input(&dir, "x.txt", "b")).unwrap();
        assert_eq!(
            v.status,
            Status::Pass,
            "write-only check must not run at pre-commit"
        );
    }

    #[test]
    fn on_filter_runs_check_subscribed_to_event() {
        let dir = tempfile::tempdir().unwrap();
        write_config(
            &dir,
            "checks:\n  g:\n    files: \"*\"\n    on: [pre-commit]\n    run: \"exit 2\"\n",
        );
        let engine = load_with_event(&dir, "pre-commit");
        let v = engine.check(file_input(&dir, "x.txt", "b")).unwrap();
        assert_eq!(
            v.status,
            Status::Block,
            "a pre-commit check must run at pre-commit"
        );
    }

    #[test]
    fn pre_commit_runs_check_once_over_the_set() {
        let dir = tempfile::tempdir().unwrap();
        // Counter: each invocation appends one byte to runs.txt via printf.
        // The Rust assertion below is the single source of truth for run count.
        write_config(
            &dir,
            "checks:\n  g:\n    files: \"*.rs\"\n    on: [pre-commit]\n    run: \"printf x >> $IRONLINT_ROOT/runs.txt\"\n",
        );
        touch(&dir, "a.rs");
        touch(&dir, "b.rs");
        let engine = load_with_event(&dir, "pre-commit");
        let v = engine
            .check_set(&[abs(&dir, "a.rs"), abs(&dir, "b.rs")])
            .unwrap();
        let runs = std::fs::read_to_string(dir.path().join("runs.txt")).unwrap_or_default();
        assert_eq!(
            runs.len(),
            1,
            "check must run exactly once over the set, got {runs:?}"
        );
        assert_eq!(v.status, Status::Pass);
    }

    // --- $IRONLINT_EVENT: pin the exact value a check sees, end-to-end ---

    #[test]
    fn ironlint_event_seen_by_check_is_write_for_write_dispatch() {
        // Traces the real write-lifecycle path (CheckOptions.event ->
        // run_one_check -> GateEnv.event -> $IRONLINT_EVENT), not just that
        // gate.rs forwards whatever string it's given.
        let dir = tempfile::tempdir().unwrap();
        write_config(
            &dir,
            "checks:\n  g:\n    files: \"*\"\n    run: \"[ \\\"$IRONLINT_EVENT\\\" = write ] || exit 2\"\n",
        );
        let engine = load_with_event(&dir, "write");
        let v = engine.check(file_input(&dir, "x.txt", "body")).unwrap();
        assert_eq!(
            v.status,
            Status::Pass,
            "check must see IRONLINT_EVENT=write on the write dispatch path"
        );
    }

    #[test]
    fn ironlint_event_seen_by_check_is_pre_commit_for_pre_commit_dispatch() {
        // Same, but through the pre-commit/set dispatch path (check_set),
        // which builds its own GateEnv independently of run_one_check.
        let dir = tempfile::tempdir().unwrap();
        write_config(
            &dir,
            "checks:\n  g:\n    files: \"*.rs\"\n    on: [pre-commit]\n    run: \"[ \\\"$IRONLINT_EVENT\\\" = pre-commit ] || exit 2\"\n",
        );
        touch(&dir, "a.rs");
        let engine = load_with_event(&dir, "pre-commit");
        let v = engine.check_set(&[abs(&dir, "a.rs")]).unwrap();
        assert_eq!(
            v.status,
            Status::Pass,
            "check must see IRONLINT_EVENT=pre-commit on the pre-commit dispatch path"
        );
    }

    // --- Phase 2: steps fail-fast ---

    #[test]
    fn steps_fail_fast_on_first_blocking_step() {
        // step 1 passes (exit 0), step 2 blocks (exit 2),
        // step 3 must NOT run. Use a sentinel file to prove step 3 was skipped.
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".ironlint.yml",
            "checks:\n  g:\n    files: \"*\"\n    steps:\n      - run: \"true\"\n      - name: blocker\n        run: \"echo nope; exit 2\"\n      - run: \"touch ran3.txt\"\n",
        );
        let target = write(dir.path(), "x.txt", "body");
        let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
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

    // --- IRONLINT_TMPFILE materialization ---

    #[test]
    fn tmpfile_materialized_with_content_ext_and_cleaned() {
        let dir = TempDir::new().unwrap();
        // Check copies $IRONLINT_TMPFILE to a stable capture path, asserts the .rs ext, then passes.
        write_config(&dir,
            "checks:\n  cap:\n    files: \"**/*.rs\"\n    run: \"case \\\"$IRONLINT_TMPFILE\\\" in *.rs) cat \\\"$IRONLINT_TMPFILE\\\" > \\\"$IRONLINT_ROOT/captured.txt\\\"; exit 0;; *) exit 2;; esac\"\n");
        let engine = load_with_event(&dir, "write");
        let path = dir.path().join("src").join("a.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "OLD").unwrap();
        let report = engine
            .check_with_explain(CheckInput::File {
                path: path.clone(),
                content: "PROPOSED-NEW".to_string(),
            })
            .unwrap();
        assert_eq!(report.verdict.status, Status::Pass);
        // The captured bytes are the PROPOSED content (not the OLD on-disk bytes).
        assert_eq!(
            std::fs::read_to_string(dir.path().join("captured.txt")).unwrap(),
            "PROPOSED-NEW"
        );
        // The temp file is gone (cleanup), but its sibling source file remains.
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("ironlint-tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temp file leaked: {leftovers:?}");
    }

    #[test]
    fn tmpfile_not_created_when_unreferenced() {
        let dir = TempDir::new().unwrap();
        write_config(
            &dir,
            "checks:\n  g:\n    files: \"**/*.rs\"\n    run: \"! grep -q TODO\"\n",
        );
        let engine = load_with_event(&dir, "write");
        let path = dir.path().join("a.rs");
        std::fs::write(&path, "fine").unwrap();
        let _ = engine
            .check_with_explain(CheckInput::File {
                path: path.clone(),
                content: "fine".into(),
            })
            .unwrap();
        let any_tmp = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("ironlint-tmp-"));
        assert!(
            !any_tmp,
            "no temp file should exist for an unreferenced check"
        );
    }

    #[test]
    fn tmpfile_unset_on_pre_commit() {
        let dir = TempDir::new().unwrap();
        // On pre-commit the var must be empty even though the check references it.
        write_config(&dir, "checks:\n  pc:\n    files: \"**/*.rs\"\n    on: [pre-commit]\n    run: \"test -z \\\"$IRONLINT_TMPFILE\\\"\"\n");
        let engine = load_with_event(&dir, "pre-commit");
        let path = dir.path().join("a.rs");
        std::fs::write(&path, "x").unwrap();
        let verdict = engine.check_set(&[path]).unwrap();
        assert_eq!(
            verdict.status,
            Status::Pass,
            "IRONLINT_TMPFILE must be unset on pre-commit"
        );
    }

    #[test]
    #[cfg(unix)]
    fn tmpfile_write_failure_is_internal_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        write_config(
            &dir,
            "checks:\n  cap:\n    files: \"**/*.rs\"\n    run: \"cat \\\"$IRONLINT_TMPFILE\\\"\"\n",
        );
        let engine = load_with_event(&dir, "write");
        let sub = dir.path().join("ro");
        std::fs::create_dir(&sub).unwrap();
        let path = sub.join("a.rs");
        std::fs::write(&path, "x").unwrap();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();
        let verdict = engine
            .check(CheckInput::File {
                path,
                content: "x".into(),
            })
            .unwrap();
        // restore perms so TempDir cleanup works
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(verdict.status, Status::InternalError);
    }

    #[test]
    #[cfg(unix)]
    fn tmpfile_is_created_exclusive_mode_0600() {
        use std::os::unix::fs::MetadataExt; // for .mode() — reading bits, not constructing

        // Direct call to materialize_tmpfile: the file's mode is unobservable
        // through the end-to-end path because TmpFileGuard::drop removes it
        // after the check runs (locked by the existing tmpfile-* tests). This
        // test locks only the new perms/exclusivity contract.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ironlint-tmp-probe.txt");
        let _ = std::fs::remove_file(&path);

        materialize_tmpfile(&path, "body").expect("materialize probe");

        let mode = std::fs::metadata(&path).expect("metadata").mode();
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            mode & 0o777,
            0o600,
            "tmpfile must be mode 0600, got {:o}",
            mode & 0o777
        );

        // Exclusivity (create_new/O_EXCL): a second create at the same path
        // must fail rather than clobber — the symlink-race fail-closed.
        std::fs::write(&path, "attacker").unwrap(); // pre-create the name
        let second = materialize_tmpfile(&path, "body");
        let _ = std::fs::remove_file(&path);
        assert!(
            second.is_err(),
            "materialize_tmpfile must fail (not clobber) when the path already exists"
        );
    }

    #[test]
    fn tmpfile_refuses_to_write_outside_project_root() {
        // Config dir A; separate tempdir B simulates an out-of-project path.
        // resolve_input_path bypasses its containment guard when the target
        // file doesn't exist yet (pre-write). maybe_materialize_tmpfile must
        // catch this and refuse to write the tmpfile.
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        write_config(
            &dir_a,
            "checks:\n  chk:\n    files: \"**/*.rs\"\n    run: \"cat \\\"$IRONLINT_TMPFILE\\\"\"\n",
        );
        let engine = IronLintEngine::builder()
            .with_options(CheckOptions {
                checks: std::iter::once("chk".to_string()).collect(),
                event: "write".to_string(),
                allow_external_paths: false,
                force: true,
            })
            .load(&dir_a.path().join(".ironlint.yml"))
            .unwrap();
        // Target does NOT exist — triggers the bypass branch in resolve_input_path.
        let evil = dir_b.path().join("evil.rs");
        let verdict = engine
            .check(CheckInput::File {
                path: evil,
                content: "x".into(),
            })
            .unwrap();
        assert_eq!(
            verdict.status,
            Status::InternalError,
            "should refuse to materialize tmpfile outside project root"
        );
        // No ironlint-tmp-* file should have been written in dir_b.
        let leaked: Vec<_> = std::fs::read_dir(dir_b.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("ironlint-tmp-"))
            .collect();
        assert!(
            leaked.is_empty(),
            "tmpfile leaked into external dir: {leaked:?}"
        );
    }

    #[test]
    fn tmpfile_allows_outside_write_with_allow_external_paths() {
        // Same topology as above, but allow_external_paths: true. The tmpfile
        // should be written, the check should run and see the proposed content,
        // and cleanup should leave no ironlint-tmp-* in dir_b.
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        // Check copies $IRONLINT_TMPFILE content to a capture path inside $IRONLINT_ROOT.
        write_config(
            &dir_a,
            "checks:\n  chk:\n    files: \"**/*.rs\"\n    run: \"cat \\\"$IRONLINT_TMPFILE\\\" > \\\"$IRONLINT_ROOT/captured.txt\\\"\"\n",
        );
        let engine = IronLintEngine::builder()
            .with_options(CheckOptions {
                checks: std::iter::once("chk".to_string()).collect(),
                event: "write".to_string(),
                allow_external_paths: true,
                force: true,
            })
            .load(&dir_a.path().join(".ironlint.yml"))
            .unwrap();
        let evil = dir_b.path().join("evil.rs");
        let verdict = engine
            .check(CheckInput::File {
                path: evil,
                content: "proposed".into(),
            })
            .unwrap();
        assert_eq!(
            verdict.status,
            Status::Pass,
            "allow_external_paths=true should permit the tmpfile write"
        );
        // The check captured the proposed content via $IRONLINT_TMPFILE.
        let captured = std::fs::read_to_string(dir_a.path().join("captured.txt")).unwrap();
        assert_eq!(captured, "proposed");
        // The tmpfile was cleaned up — no ironlint-tmp-* leftover in dir_b.
        let leaked: Vec<_> = std::fs::read_dir(dir_b.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("ironlint-tmp-"))
            .collect();
        assert!(
            leaked.is_empty(),
            "tmpfile leaked after cleanup: {leaked:?}"
        );
    }

    #[test]
    fn force_runs_out_of_scope_named_check() {
        let dir = TempDir::new().unwrap();
        write_config(
            &dir,
            "checks:\n  only-src:\n    files: \"src/**/*.rs\"\n    run: \"! grep -q BAD\"\n",
        );
        // File path is OUTSIDE the src/**/*.rs glob.
        let path = dir.path().join("fixtures").join("x.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "BAD").unwrap();
        let engine = IronLintEngine::builder()
            .with_options(CheckOptions {
                checks: std::iter::once("only-src".to_string()).collect(),
                event: "write".to_string(),
                allow_external_paths: false,
                force: true,
            })
            .load(&dir.path().join(".ironlint.yml"))
            .unwrap();
        let report = engine
            .check_with_explain(CheckInput::File {
                path,
                content: "BAD".into(),
            })
            .unwrap();
        // Without force it would be skipped out_of_scope → Pass. With force it fires → Block.
        assert_eq!(report.verdict.status, Status::Block);
    }

    #[test]
    fn force_does_not_bypass_disable_directive() {
        let dir = TempDir::new().unwrap();
        write_config(
            &dir,
            "checks:\n  only-src:\n    files: \"src/**/*.rs\"\n    run: \"! grep -q BAD\"\n",
        );
        let path = dir.path().join("x.rs");
        std::fs::write(&path, "BAD").unwrap();
        let engine = IronLintEngine::builder()
            .with_options(CheckOptions {
                checks: std::iter::once("only-src".to_string()).collect(),
                event: "write".to_string(),
                allow_external_paths: false,
                force: true,
            })
            .load(&dir.path().join(".ironlint.yml"))
            .unwrap();
        // Inline disable suppresses the check even under --force.
        let content = "BAD\n// ironlint-disable: only-src\n".to_string();
        let report = engine
            .check_with_explain(CheckInput::File { path, content })
            .unwrap();
        assert_eq!(report.verdict.status, Status::Pass);
    }

    // --- Task 2.5: stale $IRONLINT_TMPFILE sweep ---

    /// Backdate `path`'s mtime `secs_ago` seconds into the past, to simulate
    /// a tmpfile leaked by a run that was killed well before "now".
    fn backdate_mtime(path: &Path, secs_ago: u64) {
        let old = SystemTime::now() - Duration::from_secs(secs_ago);
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
    }

    #[test]
    fn sweep_stale_tmpfiles_removes_only_old_matching_files() {
        let dir = TempDir::new().unwrap();

        // Stale: matches the tmpfile naming prefix, mtime well past the
        // threshold — this is the leaked-file case the sweep exists for.
        let stale = dir.path().join("ironlint-tmp-1111-0-1.rs");
        std::fs::write(&stale, "leaked").unwrap();
        backdate_mtime(&stale, 2 * 60 * 60); // 2h ago

        // Fresh: matches the naming prefix but is recent — could be a
        // concurrently-running ironlint process's still-live tmpfile. Must
        // survive the sweep.
        let fresh = dir.path().join("ironlint-tmp-2222-0-2.rs");
        std::fs::write(&fresh, "still live").unwrap();

        // Unrelated: old, but the name doesn't match the tmpfile prefix.
        // Must never be touched, regardless of age.
        let unrelated = dir.path().join("real_file.rs");
        std::fs::write(&unrelated, "keep me").unwrap();
        backdate_mtime(&unrelated, 2 * 60 * 60);

        sweep_stale_tmpfiles(dir.path(), Duration::from_hours(1));

        assert!(!stale.exists(), "stale ironlint-tmp-* file must be swept");
        assert!(
            fresh.exists(),
            "fresh ironlint-tmp-* file must survive (may be a concurrent live run)"
        );
        assert!(
            unrelated.exists(),
            "non-tmpfile-pattern files must never be touched, regardless of age"
        );
    }

    #[test]
    fn sweep_stale_tmpfiles_ignores_directories_matching_the_prefix() {
        let dir = TempDir::new().unwrap();
        let weird_dir = dir.path().join("ironlint-tmp-a-dir");
        std::fs::create_dir(&weird_dir).unwrap();

        // Even with an effectively-zero threshold (everything old enough to
        // count as stale), a directory is never a sweep candidate.
        sweep_stale_tmpfiles(dir.path(), Duration::from_secs(0));

        assert!(
            weird_dir.exists(),
            "a directory matching the tmpfile prefix must never be removed"
        );
    }

    #[test]
    fn sweep_stale_tmpfiles_tolerates_missing_root() {
        // Best-effort: a root that doesn't exist (or vanished) must not panic.
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        sweep_stale_tmpfiles(&missing, Duration::from_hours(1));
    }

    #[test]
    fn maybe_materialize_tmpfile_sweeps_stale_leaks_in_its_own_nested_dir() {
        // $IRONLINT_TMPFILE is materialized as a SIBLING of the checked file
        // (its own directory), which for real source is nested — e.g.
        // crates/foo/src/ — not the config root. The load-time
        // sweep_stale_tmpfiles call only sweeps config_dir's immediate
        // entries, so it never reaches a leak sitting here. This drives the
        // real, end-to-end reclaim path (through maybe_materialize_tmpfile,
        // via a full check dispatch) and proves the nested leak is gone.
        let dir = TempDir::new().unwrap();
        write_config(
            &dir,
            "checks:\n  cap:\n    files: \"**/*.rs\"\n    run: \"cat \\\"$IRONLINT_TMPFILE\\\" > /dev/null\"\n",
        );
        let nested = dir.path().join("crates").join("foo").join("src");
        std::fs::create_dir_all(&nested).unwrap();

        // Stale: leaked ironlint-tmp-* file sitting in the checked file's
        // own (nested) directory, from a run killed well before "now" — the
        // exact leak the root-only load-time sweep misses.
        let stale = nested.join("ironlint-tmp-9999-0-1.rs");
        std::fs::write(&stale, "leaked").unwrap();
        backdate_mtime(&stale, 2 * 60 * 60); // 2h ago

        // Fresh: matches the naming prefix but is recent — could be a
        // concurrently-running ironlint process's still-live tmpfile in the
        // same directory. Must survive the sweep.
        let fresh = nested.join("ironlint-tmp-8888-0-2.rs");
        std::fs::write(&fresh, "still live").unwrap();

        // Unrelated: old, but the name doesn't match the tmpfile prefix.
        // Must never be touched, regardless of age.
        let unrelated = nested.join("lib.rs");
        std::fs::write(&unrelated, "keep me").unwrap();
        backdate_mtime(&unrelated, 2 * 60 * 60);

        let engine = load_with_event(&dir, "write");
        let path = nested.join("a.rs");
        std::fs::write(&path, "OLD").unwrap();
        let report = engine
            .check_with_explain(CheckInput::File {
                path: path.clone(),
                content: "PROPOSED".to_string(),
            })
            .unwrap();
        assert_eq!(report.verdict.status, Status::Pass);

        assert!(
            !stale.exists(),
            "stale ironlint-tmp-* file in the checked file's nested dir must be swept at materialization time"
        );
        assert!(
            fresh.exists(),
            "fresh ironlint-tmp-* file in the nested dir must survive (may be a concurrent live run)"
        );
        assert!(
            unrelated.exists(),
            "non-tmpfile-pattern files in the nested dir must never be touched, regardless of age"
        );
    }
}
