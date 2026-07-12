use crate::config::{Check, Config};
use crate::engine::{run_gate, GateEnv, GateOutcome, InternalReason};
use crate::telemetry::{LogEntry, PerCheckRecord};
use crate::verdict::{Block, GateError, Status, Verdict};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

mod path;
mod timeout;
mod tmpfile;
mod types;

pub(crate) use path::{canonicalize_through_parent, relativize};
pub(crate) use timeout::{
    resolve_timeout, resolve_timeout_secs, resolve_timeout_with_floor, IRONLINT_TIMEOUT_FLOOR_SECS,
};
pub(crate) use tmpfile::{
    check_references_arch_layers, check_references_tmpfile, materialize_tmpfile,
    sweep_stale_tmpfiles, unique_name, unique_tmp_name, TmpFileGuard, ARCH_LAYERS_PREFIX,
    STALE_TMPFILE_PREFIXES, TMPFILE_PREFIX, TMPFILE_SWEEP_MAX_AGE,
};
pub use types::{CheckExplain, CheckInput, CheckOptions, CheckReport, ExplainOutcome};
pub(crate) use types::{CheckStatus, Collected};

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
    /// Absolute path to the `ironlint` binary running this engine. Passed to
    /// every check as `$IRONLINT_BIN` so the lowered `__arch__` check can shell
    /// out to the same binary instead of relying on `PATH` (which may be
    /// missing or stale). Falls back to the bare name `ironlint` if
    /// `current_exe()` fails, preserving the pre-fix behavior in that edge case.
    bin: PathBuf,
    /// Absolute path to a manifest of sibling proposed files, read from the
    /// `IRONLINT_PROPOSED_MANIFEST` env var at load time (set by the codex
    /// hook before invoking `ironlint check`). Passed to the lowered
    /// `__arch__` check as `$IRONLINT_PROPOSED_MANIFEST` so the arch
    /// subprocess can merge cross-file imports within a single atomic patch
    /// as virtual graph nodes (Bug 1). `None` when the env var is unset
    /// (the common case — no manifest, no overlay, status quo behavior).
    proposed_manifest: Option<PathBuf>,
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
        Self::load_with_tmp(config_path, options, &std::env::temp_dir())
    }

    /// Same as [`IronLintEngine::load_with`], but the directory swept for
    /// `$IRONLINT_ARCH_LAYERS` leaks is injected rather than read from
    /// `std::env::temp_dir()`. The system temp dir is process-global: mutating
    /// `TMPDIR` in a test races every concurrent `TempDir::new()` in the suite
    /// (see the same reasoning in `gate.rs`'s env-scrub tests). Threading the
    /// path here keeps the load path's sweep call site testable without env
    /// gymnastics — and without serializing the whole module.
    fn load_with_tmp(config_path: &Path, options: CheckOptions, tmp_dir: &Path) -> Result<Self> {
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
        let mut config = crate::config::parse_file_with_extends(config_path)?;
        crate::arch::lowering::lower_architecture(&mut config)?;

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

        // Reclaim any $IRONLINT_TMPFILE or $IRONLINT_ARCH_LAYERS leak a prior
        // run left behind by dying mid-check (SIGTERM/SIGINT/SIGKILL skip
        // TmpFileGuard::drop — see its docstring). Best-effort and age-gated,
        // so it can never step on a tmpfile a concurrently-running ironlint
        // process still owns.
        sweep_stale_tmpfiles(&config_dir_canon, TMPFILE_SWEEP_MAX_AGE);
        // $IRONLINT_ARCH_LAYERS lives in the system temp dir, not the project
        // tree, so sweep that too. The same prefix + is_file + age gate makes
        // this safe in a shared directory. `tmp_dir` is injected (defaults to
        // `std::env::temp_dir()` in `load_with`) so the load-time sweep of the
        // system temp dir is testable without mutating process-global `TMPDIR`.
        sweep_stale_tmpfiles(tmp_dir, TMPFILE_SWEEP_MAX_AGE);

        let timeout = resolve_timeout(&config);

        // Absolute path to this ironlint binary. `current_exe()` can fail in
        // exotic cases (e.g. the executable was unlinked after startup); fall
        // back to the bare name so the lowered `__arch__` check degrades to the
        // pre-fix PATH-resolved behavior instead of crashing the engine load.
        let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ironlint"));

        // Bug 1: read the proposed-manifest path from the parent env (set by
        // the codex hook before invoking `ironlint check`). The runner owns
        // which IRONLINT_* vars flow to checks — this is the same pattern used
        // for `IRONLINT_BIN` (current_exe). `None` when unset (common case).
        let proposed_manifest = std::env::var_os("IRONLINT_PROPOSED_MANIFEST").map(PathBuf::from);

        Ok(Self {
            config,
            config_dir,
            config_dir_canon,
            options,
            scope_matchers,
            timeout,
            bin,
            proposed_manifest,
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

    /// Materialize `$IRONLINT_ARCH_LAYERS` for any check that references it.
    /// Unlike `$IRONLINT_TMPFILE`, the layers file is available at both
    /// lifecycles (write and pre-commit) and is derived from the stashed config
    /// YAML, not the proposed file content. Lives in the system temp directory
    /// so it never mixes with project source.
    fn maybe_materialize_arch_layers(
        &self,
        check: &Check,
    ) -> std::io::Result<Option<TmpFileGuard>> {
        if !check_references_arch_layers(check) {
            return Ok(None);
        }
        let Some(yaml) = self.config.arch_layers_yaml.as_ref() else {
            return Ok(None);
        };
        let name = unique_name(ARCH_LAYERS_PREFIX, Some("yml"));
        let path = std::env::temp_dir().join(name);
        materialize_tmpfile(&path, yaml)?;
        Ok(Some(TmpFileGuard { path }))
    }

    /// Build the human-readable `GateError.detail` string for an internal error:
    /// names the (truncated) run command and, for timeouts, the effective
    /// timeout that fired. One line.
    fn detail_for(reason: &InternalReason, run: &str, timeout: Duration) -> String {
        const MAX_RUN_LEN: usize = 80;
        let run_trunc = if run.len() > MAX_RUN_LEN {
            // Step back to the nearest char boundary at or below MAX_RUN_LEN
            // so a multibyte UTF-8 codepoint straddling the cut isn't split
            // (a naive `&run[..MAX_RUN_LEN]` byte slice panics on that case).
            // Keep the last whole codepoint whose end byte ≤ MAX_RUN_LEN.
            let cut = run
                .char_indices()
                .map(|(i, c)| i + c.len_utf8())
                .take_while(|end| *end <= MAX_RUN_LEN)
                .last()
                .unwrap_or(0);
            format!("{}…", &run[..cut])
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
        let arch = match self.maybe_materialize_arch_layers(check) {
            Ok(a) => a,
            Err(e) => {
                return CheckStatus::Error {
                    step: Some("<arch-layers>".to_string()),
                    reason: format!("arch_layers_write_failed:{e}"),
                    detail: None,
                    elapsed: 0,
                }
            }
        };
        let abs_buf = abs.to_path_buf();
        let env = GateEnv {
            file: Some(abs),
            files: std::slice::from_ref(&abs_buf),
            root: &self.config_dir_canon,
            event: &self.options.event,
            tmpfile: tmp.as_ref().map(|g| g.path.as_path()),
            arch_layers: arch.as_ref().map(|g| g.path.as_path()),
            bin: &self.bin,
            proposed_manifest: self.proposed_manifest.as_deref(),
        };
        self.run_steps(check, &env, Some(content.as_bytes()))
        // `tmp` and `arch` drop here → temp files removed.
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
            let arch = match self.maybe_materialize_arch_layers(check) {
                Ok(a) => a,
                Err(e) => {
                    let status = CheckStatus::Error {
                        step: Some("<arch-layers>".to_string()),
                        reason: format!("arch_layers_write_failed:{e}"),
                        detail: None,
                        elapsed: 0,
                    };
                    collected.absorb(check_id, None, status, false);
                    continue;
                }
            };
            let env = GateEnv {
                file: None,
                files: &matched,
                root: &self.config_dir_canon,
                event: &self.options.event,
                tmpfile: None,
                arch_layers: arch.as_ref().map(|g| g.path.as_path()),
                bin: &self.bin,
                proposed_manifest: self.proposed_manifest.as_deref(),
            };
            let status = self.run_steps(check, &env, None);
            // `arch` drops here → temp file removed.
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

        sweep_stale_tmpfiles(dir.path(), Duration::from_secs(60 * 60));

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
        sweep_stale_tmpfiles(&missing, Duration::from_secs(60 * 60));
    }

    #[test]
    fn sweep_stale_tmpfiles_removes_old_arch_layers_files() {
        // Bug 8: SIGKILL skips TmpFileGuard::drop for $IRONLINT_ARCH_LAYERS,
        // leaking ironlint-arch-* files in the system temp directory. The
        // sweep must reclaim them the same way it reclaims ironlint-tmp-*.
        let dir = TempDir::new().unwrap();

        let stale = dir.path().join("ironlint-arch-1111-0-1.yml");
        std::fs::write(&stale, "layers:\n").unwrap();
        backdate_mtime(&stale, 2 * 60 * 60); // 2h ago

        let unrelated = dir.path().join("not-ironlint-anything.yml");
        std::fs::write(&unrelated, "keep me").unwrap();
        backdate_mtime(&unrelated, 2 * 60 * 60);

        sweep_stale_tmpfiles(dir.path(), Duration::from_secs(60 * 60));

        assert!(!stale.exists(), "stale ironlint-arch-* file must be swept");
        assert!(
            unrelated.exists(),
            "non-matching files must never be touched, regardless of age"
        );
    }

    #[test]
    fn sweep_stale_tmpfiles_keeps_fresh_arch_layers_files() {
        // A fresh ironlint-arch-* file could belong to a concurrently-running
        // ironlint process; the age gate must keep it.
        let dir = TempDir::new().unwrap();

        let fresh = dir.path().join("ironlint-arch-2222-0-2.yml");
        std::fs::write(&fresh, "layers:\n").unwrap();

        sweep_stale_tmpfiles(dir.path(), Duration::from_secs(60 * 60));

        assert!(
            fresh.exists(),
            "fresh ironlint-arch-* file must survive (may be a concurrent live run)"
        );
    }

    #[test]
    fn sweep_stale_tmpfiles_ignores_directories_matching_arch_layers_prefix() {
        let dir = TempDir::new().unwrap();
        let weird_dir = dir.path().join("ironlint-arch-a-dir");
        std::fs::create_dir(&weird_dir).unwrap();

        // Even with a zero threshold, a directory is never a sweep candidate.
        sweep_stale_tmpfiles(dir.path(), Duration::from_secs(0));

        assert!(
            weird_dir.exists(),
            "a directory matching the arch-layers prefix must never be removed"
        );
    }

    fn load_engine_with_tmp(dir: &TempDir, tmp_dir: &Path) -> IronLintEngine {
        // Mirrors `load_with_event` but drives the real `load_with_tmp` path
        // with an injected system-temp dir, so the load-time sweep call site
        // (runner.rs `sweep_stale_tmpfiles(tmp_dir, ...)`) is exercised
        // end-to-end without mutating process-global `TMPDIR`.
        IronLintEngine::load_with_tmp(
            &dir.path().join(".ironlint.yml"),
            CheckOptions {
                event: "write".to_string(),
                ..Default::default()
            },
            tmp_dir,
        )
        .unwrap()
    }

    #[test]
    fn load_sweeps_stale_arch_layers_file_in_system_temp_dir() {
        // Bug 8 / audit item 2: the load-time sweep at the `sweep_stale_tmpfiles`
        // call site (runner.rs `sweep_stale_tmpfiles(tmp_dir, ...)`) is the
        // reclaim path for `$IRONLINT_ARCH_LAYERS` leaks left in the SYSTEM
        // temp dir by a SIGKILLed prior run. The 6 unit tests of the sweep
        // function call it directly, so they pass even if this call site were
        // deleted. This test drives the real `IronLintEngine::load` path.
        //
        // Discrimination: the stale file lives in a SEPARATE temp dir from the
        // config dir, so the config_dir sweep (line 693) cannot reclaim it —
        // only the system-temp-dir sweep at the injected `tmp_dir` can. Delete
        // that one line and the stale file survives → test fails for the right
        // reason, not a tautology.
        let config_dir = TempDir::new().unwrap();
        write_config(
            &config_dir,
            "checks:\n  noop:\n    files: \"**/*.rs\"\n    run: \"true\"\n",
        );

        // The injected system-temp dir: distinct from config_dir so the
        // config_dir sweep can't cover for a deleted system-temp sweep.
        let tmp_dir = TempDir::new().unwrap();

        // Stale: a leaked ironlint-arch-* file from a killed prior run, backdated
        // past the sweep's 1h threshold.
        let stale = tmp_dir.path().join("ironlint-arch-1111-0-1.yml");
        std::fs::write(&stale, "layers:\n").unwrap();
        backdate_mtime(&stale, 2 * 60 * 60); // 2h ago

        // Fresh: same prefix, recent mtime — a concurrently-running ironlint
        // process's still-live $IRONLINT_ARCH_LAYERS. Must survive the sweep.
        let fresh = tmp_dir.path().join("ironlint-arch-2222-0-2.yml");
        std::fs::write(&fresh, "layers:\n").unwrap();

        // Unrelated: old, but the name doesn't match the arch-layers prefix.
        // Must never be touched, regardless of age.
        let unrelated = tmp_dir.path().join("someone-elses-cache.yml");
        std::fs::write(&unrelated, "keep me").unwrap();
        backdate_mtime(&unrelated, 2 * 60 * 60);

        let _engine = load_engine_with_tmp(&config_dir, tmp_dir.path());

        assert!(
            !stale.exists(),
            "stale ironlint-arch-* file in the system temp dir must be swept at engine load"
        );
        assert!(
            fresh.exists(),
            "fresh ironlint-arch-* file must survive (may be a concurrent live run)"
        );
        assert!(
            unrelated.exists(),
            "non-matching files in the system temp dir must never be touched, regardless of age"
        );
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

    #[test]
    fn detail_for_truncates_multibyte_run_at_char_boundary() {
        // A run command > MAX_RUN_LEN (80 bytes) whose byte-80 position lands
        // inside a multibyte UTF-8 codepoint. The naive `&run[..80]` byte
        // slice panics here; the truncation must step back to the nearest
        // char boundary so the detail string is valid UTF-8 (and the
        // InternalError path doesn't panic instead of returning a verdict).
        // 78 ASCII bytes, then a 4-byte emoji (🚀) straddling bytes 78..82,
        // so byte 80 falls mid-codepoint.
        let run = "#".repeat(78) + "🚀" + "tail-here";
        assert!(
            run.len() > 80,
            "fixture must exceed the 80-byte truncation limit; got {}",
            run.len()
        );
        let detail =
            IronLintEngine::detail_for(&InternalReason::NotFound, &run, Duration::from_secs(30));
        // Must not panic (the slice would have), must end in the ellipsis,
        // and the prefix must be valid UTF-8 ending on a char boundary.
        assert!(
            detail.ends_with('…'),
            "truncated detail must end in ellipsis; got: {detail:?}"
        );
        let body = detail.strip_suffix('…').unwrap();
        let truncated = body.strip_prefix("not_found running: ").unwrap();
        // Truncated portion must be ≤80 bytes AND valid UTF-8 (char-aligned).
        assert!(
            truncated.len() <= 80,
            "truncated run must be ≤80 bytes; got {} ({truncated:?})",
            truncated.len()
        );
        assert!(
            truncated.chars().all(|_| true),
            "truncated run must be valid UTF-8 (char-boundary-aligned)"
        );
        // The emoji must NOT appear at the cut — it straddled the boundary.
        assert!(
            !truncated.contains('🚀'),
            "the multibyte char straddling byte 80 must be dropped, not split: {truncated:?}"
        );
    }
}
