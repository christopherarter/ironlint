use crate::config::{Check, Config};
use crate::engine::{run_gate, GateEnv, GateOutcome, InternalReason};
use crate::telemetry::{LogEntry, PerCheckRecord};
use crate::verdict::{Status, Verdict};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::path::relativize;
use super::timeout::resolve_timeout;
use super::tmpfile::{
    check_references_arch_layers, check_references_tmpfile, materialize_tmpfile,
    sweep_stale_tmpfiles, unique_name, unique_tmp_name, TmpFileGuard, ARCH_LAYERS_PREFIX,
    TMPFILE_SWEEP_MAX_AGE,
};
use super::types::{CheckInput, CheckOptions, CheckReport, CheckStatus, Collected};

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
    pub(crate) fn load_with_tmp(
        config_path: &Path,
        options: CheckOptions,
        tmp_dir: &Path,
    ) -> Result<Self> {
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
    pub(crate) fn detail_for(
        reason: &InternalReason,
        run: &str,
        timeout: Duration,
    ) -> String {
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
