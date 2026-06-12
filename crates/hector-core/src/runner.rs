use crate::config::skip::{parse_user_global_ignore, SkipMatcher, USER_GLOBAL_IGNORE_FILENAME};
use crate::config::{Config, EngineKind, Rule};
use crate::engine::{RuleContext, RuleEngine};
use crate::telemetry::{LogEntry, PerRuleRecord};
use crate::verdict::{Status, Verdict, Violation};
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Map a config `EngineKind` to the verdict-side `Engine` used in telemetry.
fn engine_kind_to_verdict_engine(kind: EngineKind) -> crate::verdict::Engine {
    match kind {
        EngineKind::Script => crate::verdict::Engine::Script,
        EngineKind::Ast => crate::verdict::Engine::Ast,
        EngineKind::Semantic => crate::verdict::Engine::Semantic,
        EngineKind::Session => crate::verdict::Engine::Session,
    }
}

/// Whether a rule is collected into the deferred envelope instead of
/// dispatched: true only when `emit_semantic_payload` is set and the engine
/// is `Semantic` or `Session` (`Script`/`Ast` always run).
fn should_defer(engine: EngineKind, options: &CheckOptions) -> bool {
    options.emit_semantic_payload && matches!(engine, EngineKind::Semantic | EngineKind::Session)
}

/// Render a `Severity` as the bully-compatible string the deferred
/// envelope's `severity` field carries.
fn severity_string(s: crate::config::Severity) -> String {
    match s {
        crate::config::Severity::Error => "error".into(),
        crate::config::Severity::Warning => "warning".into(),
    }
}

/// Translate deferred-context expansion failures into `__internal`
/// violations and append them to `violations`.
fn push_expansion_failures_into_violations(
    failures: &[(String, anyhow::Error)],
    path: &Path,
    violations: &mut Vec<Violation>,
) {
    for (rule_id, err) in failures {
        violations.push(Violation {
            rule_id: "__internal".to_string(),
            severity: crate::verdict::Severity::Error,
            engine: crate::verdict::Engine::Internal,
            file: path.display().to_string(),
            line: None,
            column: None,
            message: format!("deferred context expansion failed for rule `{rule_id}`: {err:#}"),
            suggestion: None,
            context: None,
        });
    }
}

/// Sweep warn-severity violations onto the deferred envelope. The CLI
/// suppresses the standard verdict JSON when it emits an envelope, so
/// without this they would never reach stdout.
fn build_deferred_warnings(verdict: &Verdict) -> Vec<crate::verdict_deferred::DeferredWarning> {
    verdict
        .violations
        .iter()
        .filter(|v| v.severity == crate::verdict::Severity::Warning)
        .map(|v| crate::verdict_deferred::DeferredWarning {
            rule_id: v.rule_id.clone(),
            engine: v.engine,
            file: v.file.clone(),
            line: v.line,
            column: v.column,
            message: v.message.clone(),
        })
        .collect()
}

/// Optional per-run knobs for `HectorEngine::check`. Plumbed via
/// `builder().with_options(...)` so the public `check` signature stays
/// stable as knobs are added.
#[derive(Debug, Clone, Default)]
pub struct CheckOptions {
    /// Restrict evaluation to these rule ids. Empty set = run all rules.
    /// The filter is enforced upstream of the dispatch pool, so filtered-out
    /// rules never enter the work queue or trigger their engine (no LLM call).
    pub rules: HashSet<String>,
    /// If true, capture per-rule outcomes for the explain report.
    pub explain: bool,
    /// When true, `semantic`/`session` rules are collected into
    /// [`CheckReport::deferred`] for an in-session subagent rather than
    /// dispatched.
    pub emit_semantic_payload: bool,
    /// Allow checking files whose canonical path falls outside `config_dir`.
    /// Off by default so wrappers can't run policy against arbitrary host
    /// files.
    pub allow_external_paths: bool,
}

/// One row of the `--explain` report. Surfaced to the CLI via
/// [`CheckReport`], kept out of the verdict JSON (whose shape is locked).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleExplain {
    pub rule_id: String,
    pub engine: EngineKind,
    pub outcome: ExplainOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplainOutcome {
    /// Rule emitted at least one violation.
    Fire,
    /// Deterministic engine returned a clean pass.
    Pass,
    /// Semantic rule reached the LLM and the LLM returned `pass`.
    Dispatched,
    /// Rule was short-circuited before engine dispatch (e.g. the diff
    /// pre-filter) or the engine returned an error.
    Skipped { reason: String },
}

/// Companion return shape for [`HectorEngine::check_with_explain`].
#[derive(Debug, Clone)]
pub struct CheckReport {
    pub verdict: Verdict,
    pub explain: Vec<RuleExplain>,
    /// Present when `emit_semantic_payload` was set and at least one
    /// semantic/session rule survived scope/skip/diff-prefilter; `None`
    /// otherwise. The CLI branches on this to emit a `DeferredVerdict` or a
    /// standard `Verdict`.
    pub deferred: Option<crate::verdict_deferred::DeferredVerdict>,
}

/// One rendered semantic prompt. `system` + `user` mirror Anthropic's
/// `/v1/messages` split; OpenAI-compat providers concatenate them.
#[derive(Debug, Clone)]
pub struct RenderedPrompt {
    pub rule_id: String,
    pub system: String,
    pub user: String,
}

/// Which rules are in scope for a file, plus any skip-pattern hit.
///
/// The read-only counterpart to `check_inner`'s scope walk: no engine runs,
/// no LLM, no telemetry. Rendered by `hector explain` / `hector guide`.
#[derive(Debug, Clone)]
pub struct ScopeOutcomes {
    /// `Some(hit)` if the file matches a built-in or user skip pattern.
    /// `explain` prints a `SKIPPED` banner first and *still* enumerates
    /// per-rule rows so the author sees the full scope picture; `guide`
    /// short-circuits to an empty list (skipped files have no applicable
    /// guidance).
    pub skip: Option<SkipHit>,
    /// One entry per rule in the resolved (extends-merged) config, in
    /// `BTreeMap` key order — same iteration order `check_inner` uses, so
    /// the explain output is deterministic and bisectable against
    /// `hector check`.
    pub rules: Vec<RuleScopeEntry>,
}

/// Which skip pattern matched the file. `pattern` is the raw glob the
/// matcher was built from — what the author would put in `skip:` to
/// reproduce or override the hit.
#[derive(Debug, Clone)]
pub struct SkipHit {
    pub pattern: String,
}

/// Per-rule scope outcome. `engine`, `severity`, and `description` are
/// mirrored here (cheap clones) so `guide` can render its
/// `<rule-id> [<severity>] <description>` line without re-borrowing.
#[derive(Debug, Clone)]
pub struct RuleScopeEntry {
    pub rule_id: String,
    pub engine: EngineKind,
    pub severity: crate::config::Severity,
    pub description: String,
    pub scope_match: ScopeMatch,
}

/// Scope-match outcome for one rule against one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeMatch {
    /// File matches the rule's scope. `glob` is the *first* scope glob
    /// that matched (deterministic — the rule's `scope:` list is iterated
    /// in author order).
    Match { glob: String },
    /// File does not match any of the rule's scope globs. `scopes` is the
    /// rule's full scope list (verbatim) so `explain` can surface them
    /// in the `skip <rule-id> scope=<globs>` line.
    NoMatch { scopes: Vec<String> },
}

/// Per-rule evaluation result, before the runner-level baseline pass.
///
/// `passed` is `Some(rule_id)` when the rule produced no emitted violations
/// (no match, no engine output, or every match suppressed by a disable
/// directive); `None` otherwise. Splitting passed/violations from one
/// `Result<Vec<Violation>>` keeps the parallel `collect` straightforward.
/// `explain` carries an optional explain row (always `None` when
/// `collect_explain` is off), produced inside `evaluate_one_rule` so the
/// parallel dispatch keeps a single per-rule output type.
struct RuleOutcome {
    violations: Vec<Violation>,
    passed: Option<String>,
    explain: Option<RuleExplain>,
    /// Per-rule telemetry line. Populated when the rule reached dispatch
    /// (or was short-circuited by the diff pre-filter); `None` when the
    /// rule was out of scope.
    record: Option<PerRuleRecord>,
}

/// Result of pre-expanding deferred rule contexts before the verdict is
/// finalised. Failures become `__internal` violations; only successes are
/// threaded into the deferred envelope.
struct DeferredExpansion<'a> {
    successes: Vec<(crate::llm::prompt::RuleRef<'a>, String, Option<String>)>,
    /// `(rule_id, error)` pairs for rules whose context could not expand.
    failures: Vec<(String, anyhow::Error)>,
}

/// The three accumulators `check_session` threads through each rule's
/// outcome, bundled so `absorb_session_outcome` takes one argument.
struct SessionAcc<'a> {
    violations: &'a mut Vec<Violation>,
    passed: &'a mut Vec<String>,
    records: &'a mut Vec<PerRuleRecord>,
}

/// Per-file inputs reused across every rule evaluation in one `check()`
/// call.
struct CheckInputs<'a> {
    match_path: &'a Path,
    path: &'a Path,
    /// `Some(s)` (even when `s` is empty) means the caller authoritatively
    /// supplied content for evaluation — a CLI `--content` PreToolUse
    /// payload, or a successful disk read in diff mode. `None` means
    /// content is genuinely unavailable (read failure in diff mode),
    /// which the AST engine surfaces as an `__internal` violation.
    /// Treating an empty `Some("")` as None would conflate "empty file
    /// is fine" with "we couldn't read the file at all," which the
    /// PreToolUse `write_file` case requires us to distinguish.
    content: Option<&'a str>,
    diff: &'a str,
    disable_map: &'a crate::disable::DisableMap,
    /// Build a `RuleExplain` row for every rule that reaches dispatch (or is
    /// short-circuited by the diff pre-filter). Out-of-scope and
    /// filter-skipped rules never enter `evaluate_one_rule`, so they don't
    /// appear in the report.
    collect_explain: bool,
}

/// Outcome of normalizing a [`CheckInput`] into the path/content/diff the
/// rule loop needs. A rejected path (outside `config_dir`) short-circuits
/// to a terminal verdict the caller returns verbatim.
enum InputResolution {
    Resolved {
        path: PathBuf,
        content: String,
        diff: String,
        content_authoritative: bool,
    },
    Rejected(Verdict),
}

/// Rules split by how they run: `selected` dispatch through the engine
/// pool, `deferred` are handed to an in-session evaluator. `explain` holds
/// the rows the deferred split contributes when `--explain` is on.
struct RulePartition<'a> {
    selected: Vec<(&'a String, &'a Rule)>,
    deferred: Vec<crate::verdict_deferred::DeferredRule>,
    explain: Vec<RuleExplain>,
}

/// Folded result of dispatching the selected rules in parallel.
#[derive(Default)]
struct DispatchOutcome {
    violations: Vec<Violation>,
    passed: Vec<String>,
    explain: Vec<RuleExplain>,
    records: Vec<PerRuleRecord>,
}

pub struct HectorEngine {
    config: Config,
    config_dir: PathBuf,
    /// Canonical form of `config_dir`, computed once at load time so
    /// `rule_matches_path` doesn't `canonicalize()` the root on every call.
    config_dir_canon: PathBuf,
    llm: Option<Box<dyn crate::llm::LlmClient>>,
    skip: SkipMatcher,
    options: CheckOptions,
    /// Per-rule `ScopeMatcher` cache, keyed by rule id and built once at load
    /// time so `rule_matches_path` never rebuilds a GlobSet per (rule, file).
    /// `BTreeMap` order mirrors `config.rules`, keeping dispatch deterministic.
    scope_matchers: BTreeMap<String, crate::config::scope::ScopeMatcher>,
}

/// Resolve the current user's home directory from environment variables.
/// Mirrors what `dirs::home_dir` does on Unix and Windows without the dep.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

/// Canonicalize `path` if it exists; otherwise walk up to the deepest
/// existing ancestor, canonicalize that, and re-append the missing tail.
/// `None` only if no ancestor exists (effectively impossible for any
/// absolute path on a mounted filesystem).
///
/// Needed for PreToolUse `--content`: the agent's `write_file` proposed
/// edit targets a path that does not exist on disk yet. Plain
/// `canonicalize` fails, but the parent (or its parent…) typically does,
/// and macOS's `/var → /private/var` symlink means the parent's
/// canonical form differs from its literal form. Resolving through the
/// parent produces a path that `strip_prefix(config_dir_canon)` can
/// actually match, which scope rules then see correctly.
fn canonicalize_through_parent(path: &std::path::Path) -> Option<PathBuf> {
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

/// Resolve `path` to a form that can be matched against a relative scope glob
/// authored in `config_dir`-relative terms.
///
/// Two fallback layers:
/// 1. `canonicalize` failure (file missing — e.g. diff mode references a path
///    not yet on disk) falls back to `canonicalize_through_parent` so that
///    `--content`-mode PreToolUse paths on macOS (`/var/...` vs.
///    `/private/var/...`) still match the canonical `config_dir`. If even
///    the ancestor walk fails, returns the original `PathBuf`.
/// 2. `strip_prefix` failure (the input resolves outside `config_dir` — e.g.
///    `hector check /etc/passwd` against a `~/proj/.hector.yml`) returns the
///    canonicalized absolute path. Bare-pattern globs in `config/scope.rs`
///    register a `**/<pattern>` form, so absolute paths can still match
///    rules like `*.py` via that fallback.
fn relativize(path: &std::path::Path, root: &std::path::Path) -> std::path::PathBuf {
    let canon_path = canonicalize_through_parent(path).unwrap_or_else(|| PathBuf::from(path));
    let canon_root = root.canonicalize().unwrap_or_else(|_| PathBuf::from(root));
    canon_path
        .strip_prefix(&canon_root)
        .map(PathBuf::from)
        .unwrap_or(canon_path)
}

pub struct HectorEngineBuilder {
    llm: Option<Box<dyn crate::llm::LlmClient>>,
    options: CheckOptions,
}

impl HectorEngineBuilder {
    pub fn new() -> Self {
        Self {
            llm: None,
            options: CheckOptions::default(),
        }
    }

    pub fn with_llm(mut self, llm: Box<dyn crate::llm::LlmClient>) -> Self {
        self.llm = Some(llm);
        self
    }

    /// Attach optional per-run knobs (rule filter, explain capture).
    pub fn with_options(mut self, options: CheckOptions) -> Self {
        self.options = options;
        self
    }

    pub fn load(self, config_path: &Path) -> Result<HectorEngine> {
        HectorEngine::load_with(config_path, self.llm, self.options)
    }
}

impl Default for HectorEngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub enum CheckInput {
    File { path: PathBuf, content: String },
    Diff { file: PathBuf, unified_diff: String },
}

/// Translate `(engine, errored, emitted)` into the explain outcome for a
/// rule that reached engine dispatch (rules filtered or short-circuited by
/// the diff pre-filter produce their own rows upstream).
///
/// * `engine_errored` → `Skipped { reason: "engine_error" }`. The rule
///   surfaced a `__internal` violation but its policy verdict is
///   indeterminate, so the explain row marks it skipped rather than
///   asserting fire/pass.
/// * `any_emitted` → `Fire`.
/// * Otherwise: `Dispatched` for semantic (LLM ran and returned clean),
///   `Pass` for deterministic engines.
fn explain_outcome_for(
    engine: EngineKind,
    engine_errored: bool,
    any_emitted: bool,
) -> ExplainOutcome {
    if engine_errored {
        ExplainOutcome::Skipped {
            reason: "engine_error".into(),
        }
    } else if any_emitted {
        ExplainOutcome::Fire
    } else if engine == EngineKind::Semantic {
        ExplainOutcome::Dispatched
    } else {
        ExplainOutcome::Pass
    }
}

impl HectorEngine {
    pub fn load(config_path: &Path) -> Result<Self> {
        Self::load_with(config_path, None, CheckOptions::default())
    }

    pub fn builder() -> HectorEngineBuilder {
        HectorEngineBuilder::new()
    }

    /// Iterator over every rule id in the loaded config. The CLI uses it to
    /// validate `--rule` arguments at the boundary, before any dispatch.
    pub fn config_rule_ids(&self) -> impl Iterator<Item = &str> {
        self.config.rules.keys().map(|k| k.as_str())
    }

    /// Replace the rule-id filter on an already-loaded engine, so the CLI can
    /// load once, validate `--rule` against the config, then store the
    /// validated set rather than loading twice. Library callers set the filter
    /// at build time via [`HectorEngineBuilder::with_options`].
    pub fn set_rule_filter(&mut self, rules: HashSet<String>) {
        self.options.rules = rules;
    }

    /// Look up a rule by id from the loaded config — used to resolve
    /// `DeferredRule` ids back to their full definitions.
    pub fn config_rule(&self, id: &str) -> Option<&crate::config::Rule> {
        self.config.rules.get(id)
    }

    /// Read-only scope walk: the skip-pattern hit (if any) and a per-rule
    /// scope outcome for every rule in the resolved config. No engine runs,
    /// no LLM is constructed, no telemetry is written.
    ///
    /// Used by `hector explain <file>` and `hector guide <file>` so they
    /// share one source of truth for "what's in scope for this path?"
    /// with `hector check`'s dispatch loop. The path is relativized
    /// against the config dir using the same fallback rules as the
    /// regular check path, so an absolute `/etc/passwd` and a relative
    /// `etc/passwd` produce the same per-rule outcome shape.
    pub fn scope_outcomes(&self, file: &std::path::Path) -> ScopeOutcomes {
        let match_path = relativize(file, &self.config_dir);

        // The load-time skip matcher already unions built-ins + project skip +
        // user-global ignore, so it is the single source of truth here too.
        let skip = self
            .skip
            .matched_pattern(&match_path)
            .map(|pattern| SkipHit {
                pattern: pattern.to_string(),
            });

        let mut rules: Vec<RuleScopeEntry> = Vec::with_capacity(self.config.rules.len());
        for (rule_id, rule) in &self.config.rules {
            let matched = self
                .scope_matchers
                .get(rule_id)
                .and_then(|m| m.matched_pattern(&match_path).map(str::to_string));
            let scope_match = match matched {
                Some(glob) => ScopeMatch::Match { glob },
                None => ScopeMatch::NoMatch {
                    scopes: rule.scope.clone(),
                },
            };
            rules.push(RuleScopeEntry {
                rule_id: rule_id.clone(),
                engine: rule.engine,
                severity: rule.severity,
                description: rule.description.clone(),
                scope_match,
            });
        }
        ScopeOutcomes { skip, rules }
    }

    /// Resolve an input path argument against the engine's config dir.
    ///
    /// Absolute paths pass through unchanged. Relative paths are joined
    /// onto `self.config_dir` so a diff produced by an editor (which
    /// carries `+++ b/<rel>` paths) resolves to the same on-disk file
    /// regardless of the agent's CWD.
    ///
    /// By default, returns `Err` when the canonicalized path falls outside
    /// `config_dir`; `allow_external_paths` opts in. Files that can't be
    /// canonicalized (e.g. diff-mode paths not yet on disk) skip the
    /// outside-check and return the raw resolved path unchanged.
    pub fn resolve_input_path(&self, p: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
        let resolved = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.config_dir.join(p)
        };
        // Canonicalize if possible. Files referenced by --diff may not
        // yet exist on disk; in that case skip the outside-check (no
        // harm done, the file read will fail anyway).
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

    /// Match a path against a rule's scope, using the unified `relativize`
    /// step shared with `check_inner`.
    ///
    /// Looks up the pre-built `ScopeMatcher` from the load-time cache instead
    /// of constructing a GlobSet, and uses `config_dir_canon` to avoid a
    /// `canonicalize` syscall on the root. Relative paths are matched directly
    /// (already config-dir-relative); an unknown rule id returns `false`.
    pub fn rule_matches_path(&self, rule_id: &str, file: &std::path::Path) -> bool {
        // Relative paths are already config-dir-relative — skip the
        // canonicalize syscall. Absolute paths (e.g. adapter payloads) go
        // through the strip-prefix dance to match correctly.
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
            .get(rule_id)
            .map(|m| m.matches(&match_path))
            .unwrap_or(false)
    }

    fn load_with(
        config_path: &Path,
        llm_override: Option<Box<dyn crate::llm::LlmClient>>,
        options: CheckOptions,
    ) -> Result<Self> {
        // Debug hook: counts engine loads per process. Gated on the env var so
        // it is invisible in production; integration tests set it to assert
        // that `hector check` loads the engine exactly once.
        static LOAD_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = LOAD_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if std::env::var("HECTOR_DEBUG_LOAD_COUNT").is_ok() {
            eprintln!("hector_load_count={n}");
        }

        // `resolve_trusted` verifies the trust block of the root and every
        // transitive ancestor reachable through `extends:`. This is the only
        // gate before `script:` rules may run, so the trust chain must be
        // verified end-to-end here. It also detects schema v1 (P2-11) before
        // trust verify and surfaces a `hector migrate` hint.
        let config = crate::config::extends::resolve_trusted(config_path)?;

        // Validate every rule's scope by constructing the matcher up front,
        // and cache it so `rule_matches_path` never rebuilds a GlobSet —
        // one build per rule at load time instead of per (rule, file).
        let mut scope_matchers: BTreeMap<String, crate::config::scope::ScopeMatcher> =
            BTreeMap::new();
        for (rule_id, rule) in &config.rules {
            let matcher = crate::config::scope::ScopeMatcher::new(&rule.scope)
                .with_context(|| format!("rule `{rule_id}` has invalid scope glob"))?;
            scope_matchers.insert(rule_id.clone(), matcher);
        }

        // If no explicit override, auto-construct from config.llm.
        let llm = match llm_override {
            Some(client) => Some(client),
            None => match &config.llm {
                Some(cfg) => crate::llm::build_from_config(cfg)?,
                None => None,
            },
        };

        // Path::parent() returns Some("") for a bare relative filename
        // (e.g. ".hector.yml"), not None — filter that out so config_dir is
        // always a usable directory.
        let config_dir = config_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        let mut skip_extras = config.skip.clone();
        if let Some(home) = home_dir() {
            let ignore_path = home.join(USER_GLOBAL_IGNORE_FILENAME);
            if let Ok(raw) = std::fs::read_to_string(&ignore_path) {
                skip_extras.extend(parse_user_global_ignore(&raw));
            }
        }
        let skip = SkipMatcher::with_built_ins(&skip_extras)?;

        // Cache the canonical config_dir once so rule_matches_path never
        // calls canonicalize() on the root per invocation.
        let config_dir_canon = config_dir
            .canonicalize()
            .unwrap_or_else(|_| config_dir.clone());

        Ok(Self {
            config,
            config_dir,
            config_dir_canon,
            llm,
            skip,
            options,
            scope_matchers,
        })
    }

    /// Build a rayon thread pool sized by the precedence:
    /// `HECTOR_MAX_WORKERS` env → `config.execution.max_workers` →
    /// `min(8, num_cpus::get())`. A zero or unparseable env value falls back
    /// to the next layer; a zero config value falls back to the default.
    /// The final value is clamped to `>= 1` because `num_threads(0)` panics
    /// in rayon.
    fn execution_pool(&self) -> rayon::ThreadPool {
        let env = std::env::var("HECTOR_MAX_WORKERS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0);
        let cfg = self
            .config
            .execution
            .as_ref()
            .map(|e| e.max_workers)
            .filter(|n| *n > 0);
        let default = std::cmp::min(8, num_cpus::get().max(1));
        let n = env.or(cfg).unwrap_or(default).max(1);
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .expect("rayon pool construction must not fail")
    }

    /// Evaluate a single rule against a single file. Borrows everything so
    /// `check`'s parallel dispatch can `par_iter().map(…)` over it; the owned
    /// output fields merge cleanly post-iteration.
    ///
    /// When `collect_explain` is set, the outcome carries a `RuleExplain` row
    /// (fire / pass / dispatched / skipped). Out-of-scope rules return early
    /// with `explain: None`, since they don't appear in the report.
    fn evaluate_one_rule(
        &self,
        rule_id: &str,
        rule: &Rule,
        inputs: &CheckInputs<'_>,
    ) -> RuleOutcome {
        // Use the load-time cached matcher — no GlobSet rebuild per call.
        if !self.rule_matches_path(rule_id, inputs.match_path) {
            return RuleOutcome {
                violations: vec![],
                passed: None,
                explain: None,
                record: None,
            };
        }
        // Short-circuit semantic dispatch when the diff can't plausibly match
        // (see `try_semantic_skip`). The reason string feeds the explain row,
        // so authors see the same string the telemetry recorded.
        if let Some(reason) = self.try_semantic_skip(rule_id, rule, inputs.path, inputs.diff) {
            let explain = inputs.collect_explain.then(|| RuleExplain {
                rule_id: rule_id.to_string(),
                engine: rule.engine,
                outcome: ExplainOutcome::Skipped {
                    reason: reason.clone(),
                },
            });
            let record = Some(PerRuleRecord {
                rule_id: rule_id.to_string(),
                engine: engine_kind_to_verdict_engine(rule.engine),
                status: Status::Pass,
                elapsed_ms: 0,
                reason: Some(reason),
            });
            return RuleOutcome {
                violations: vec![],
                passed: Some(rule_id.to_string()),
                explain,
                record,
            };
        }
        let ctx = RuleContext {
            rule_id,
            rule,
            file: inputs.path,
            // `inputs.content` already encodes authoritative-vs-missing;
            // pass it through verbatim. Collapsing an explicitly-empty
            // PreToolUse payload to `None` would make the AST engine refuse
            // it with an `__internal` violation.
            content: inputs.content,
            diff: if inputs.diff.is_empty() {
                None
            } else {
                Some(inputs.diff)
            },
            cwd: &self.config_dir,
            llm: self.llm.as_deref(),
        };
        let rule_start = Instant::now();
        let outcome: Result<Vec<Violation>> = match rule.engine {
            EngineKind::Script => crate::engine::script::ScriptEngine.run(&ctx),
            EngineKind::Ast => crate::engine::ast::AstEngine.run(&ctx),
            EngineKind::Semantic => crate::engine::semantic::SemanticEngine.run(&ctx),
            // Session is dispatched via `check_session`, not the per-file
            // path; treat it as a pass here.
            _ => Ok(Vec::new()),
        };
        let rule_elapsed = rule_start.elapsed().as_millis() as u64;
        // A semantic rule that reached the LLM and produced a result emits a
        // SemanticVerdict telemetry line; errors don't (they surface as
        // engine_error in the per-rule record).
        if rule.engine == EngineKind::Semantic {
            if let Ok(ref vs) = outcome {
                let verdict_str = if vs.is_empty() { "pass" } else { "violation" };
                self.append_semantic_verdict(
                    rule_id,
                    Some(&inputs.path.display().to_string()),
                    verdict_str,
                );
            }
        }
        Self::merge_engine_outcome(rule_id, rule.engine, inputs, outcome, rule_elapsed)
    }

    /// Emit a SemanticVerdict telemetry line, used by `evaluate_one_rule`'s
    /// semantic arm and by `check_session` when a session rule reaches the
    /// LLM. Best-effort: failures warn to stderr.
    fn append_semantic_verdict(&self, rule_id: &str, file: Option<&str>, verdict_str: &str) {
        let entry = LogEntry::SemanticVerdict {
            ts: chrono::Utc::now().to_rfc3339(),
            rule: rule_id.to_string(),
            verdict: verdict_str.into(),
            file: file.map(str::to_string),
        };
        if let Err(e) = crate::telemetry::append(&self.config_dir.join(".hector/log.jsonl"), &entry)
        {
            eprintln!("hector: telemetry append failed: {e:#}");
        }
    }

    /// Post-process the engine's `Result<Vec<Violation>>` into a `RuleOutcome`:
    /// apply disable-directive suppression and convert engine errors into
    /// `Engine::Internal` violations. When `collect_explain` is set, the
    /// outcome's `RuleExplain` row is derived from
    /// `(engine_errored, any_emitted, engine)` by [`explain_outcome_for`].
    fn merge_engine_outcome(
        rule_id: &str,
        engine: EngineKind,
        inputs: &CheckInputs<'_>,
        outcome: Result<Vec<Violation>>,
        elapsed: u64,
    ) -> RuleOutcome {
        let verdict_engine = engine_kind_to_verdict_engine(engine);
        match outcome {
            // The engine may return many violations (AST emits one per
            // match). Walk them, apply per-violation disable directives, and
            // record the rule as passed only if every match was suppressed
            // (or there were none to begin with).
            Ok(vs) if vs.is_empty() => {
                let explain = inputs.collect_explain.then(|| RuleExplain {
                    rule_id: rule_id.to_string(),
                    engine,
                    outcome: explain_outcome_for(engine, false, false),
                });
                RuleOutcome {
                    violations: vec![],
                    passed: Some(rule_id.to_string()),
                    explain,
                    record: Some(PerRuleRecord {
                        rule_id: rule_id.to_string(),
                        engine: verdict_engine,
                        status: Status::Pass,
                        elapsed_ms: elapsed,
                        reason: None,
                    }),
                }
            }
            Ok(vs) => Self::apply_disables(rule_id, engine, inputs, vs, elapsed),
            Err(e) => {
                // Engine runtime errors are Engine::Internal, not
                // Engine::Trust — trust failures halt at load time and never
                // reach this arm.
                let v = Violation {
                    rule_id: format!("{rule_id}__internal"),
                    severity: crate::verdict::Severity::Error,
                    engine: crate::verdict::Engine::Internal,
                    file: inputs.path.display().to_string(),
                    line: None,
                    column: None,
                    message: format!("{e:#}"),
                    suggestion: None,
                    context: None,
                };
                let explain = inputs.collect_explain.then(|| RuleExplain {
                    rule_id: rule_id.to_string(),
                    engine,
                    outcome: explain_outcome_for(engine, true, false),
                });
                RuleOutcome {
                    violations: vec![v],
                    passed: None,
                    explain,
                    record: Some(PerRuleRecord {
                        rule_id: rule_id.to_string(),
                        engine: verdict_engine,
                        status: Status::Block,
                        elapsed_ms: elapsed,
                        reason: Some("engine_error".into()),
                    }),
                }
            }
        }
    }

    /// Walk the engine's emitted violations, dropping any that match a
    /// `hector-disable:` directive. P1-2: script/semantic emit file-level
    /// violations with `line: None`, so we honour file-wide disable
    /// directives anywhere in the file in that case.
    fn apply_disables(
        rule_id: &str,
        engine: EngineKind,
        inputs: &CheckInputs<'_>,
        vs: Vec<Violation>,
        elapsed: u64,
    ) -> RuleOutcome {
        let mut kept: Vec<Violation> = Vec::new();
        let mut any_emitted = false;
        let mut any_disabled = false;
        for v in vs {
            let disabled = match v.line {
                Some(line) => inputs.disable_map.is_disabled(line, rule_id),
                None => inputs.disable_map.is_disabled_file_wide(rule_id),
            };
            if disabled {
                any_disabled = true;
                continue;
            }
            kept.push(v);
            any_emitted = true;
        }
        let verdict_engine = engine_kind_to_verdict_engine(engine);
        let (status, reason) = if any_emitted {
            let sev = kept[0].severity;
            let s = match sev {
                crate::verdict::Severity::Error => Status::Block,
                crate::verdict::Severity::Warning => Status::Warn,
            };
            (s, None)
        } else if any_disabled {
            (Status::Pass, Some("disabled".to_string()))
        } else {
            (Status::Pass, None)
        };
        let passed = if any_emitted {
            None
        } else {
            // Every match was suppressed by a disable directive — treat the
            // rule as passing for this file so it shows up in
            // `passed_checks` and telemetry.
            Some(rule_id.to_string())
        };
        let explain = inputs.collect_explain.then(|| RuleExplain {
            rule_id: rule_id.to_string(),
            engine,
            outcome: explain_outcome_for(engine, false, any_emitted),
        });
        RuleOutcome {
            violations: kept,
            passed,
            explain,
            record: Some(PerRuleRecord {
                rule_id: rule_id.to_string(),
                engine: verdict_engine,
                status,
                elapsed_ms: elapsed,
                reason,
            }),
        }
    }

    /// Run the loaded rules against `input` and return the verdict.
    pub fn check(&self, input: CheckInput) -> Result<Verdict> {
        self.check_inner(input, false).map(|r| r.verdict)
    }

    /// Like [`Self::check`], but returns per-rule explain rows when the
    /// engine was built with `CheckOptions { explain: true, .. }`. With
    /// explain off the returned list is empty.
    pub fn check_with_explain(&self, input: CheckInput) -> Result<CheckReport> {
        self.check_inner(input, self.options.explain)
    }

    /// Normalize a [`CheckInput`] into the path, content, and diff the rule
    /// loop evaluates against.
    ///
    /// Both modes resolve the caller's path through `config_dir`, so a
    /// relative path from an editor running in a different CWD lands on the
    /// right on-disk file. A path outside `config_dir` is rejected (unless
    /// `allow_external_paths`); in file mode that becomes a terminal
    /// `__internal` verdict, in diff mode a warning that continues with the
    /// original path.
    ///
    /// `content_authoritative` distinguishes "caller supplied this content"
    /// (file mode — even an empty string is a real target) from "we read it
    /// off disk and the read failed" (diff mode). A failed read yields
    /// `false` so AST/semantic engines surface `__internal` rather than
    /// silently passing on missing content.
    fn resolve_check_input(&self, input: CheckInput, start: Instant) -> InputResolution {
        match input {
            CheckInput::File { path, content } => match self.resolve_input_path(&path) {
                Ok(resolved) => {
                    let diff = self.synthesize_file_diff(&resolved, &content);
                    InputResolution::Resolved {
                        path: resolved,
                        content,
                        diff,
                        content_authoritative: true,
                    }
                }
                Err(e) => {
                    let v = Violation {
                        rule_id: "__internal".to_string(),
                        severity: crate::verdict::Severity::Error,
                        engine: crate::verdict::Engine::Internal,
                        file: path.display().to_string(),
                        line: None,
                        column: None,
                        message: format!("{e:#}"),
                        suggestion: None,
                        context: None,
                    };
                    let elapsed = start.elapsed().as_millis() as u64;
                    InputResolution::Rejected(Verdict::from_violations(vec![v], vec![], elapsed))
                }
            },
            CheckInput::Diff { file, unified_diff } => {
                // Diff mode runs after the agent's edit has landed, so the
                // on-disk read is the post-edit content AST/semantic rules
                // and disable directives need.
                let resolved = self.resolve_input_path(&file).unwrap_or_else(|e| {
                    eprintln!(
                        "hector: path rejected for diff check ({e}); \
                         continuing with original path",
                    );
                    file
                });
                let (content, content_authoritative) = match std::fs::read_to_string(&resolved) {
                    Ok(s) => (s, true),
                    Err(e) => {
                        eprintln!(
                            "hector: failed to read {} for diff check ({e}); \
                             rules requiring file content will be skipped",
                            resolved.display()
                        );
                        (String::new(), false)
                    }
                };
                InputResolution::Resolved {
                    path: resolved,
                    content,
                    diff: unified_diff,
                    content_authoritative,
                }
            }
        }
    }

    /// Build diff evidence for a pre-write/file-content check by comparing
    /// the caller-supplied content with the current on-disk file. If the file
    /// does not exist yet, the proposed content is rendered as an addition.
    fn synthesize_file_diff(&self, path: &Path, content: &str) -> String {
        let old = std::fs::read_to_string(path).ok();
        let match_path = relativize(path, &self.config_dir);
        crate::diff::synthesize_unified(&match_path, old.as_deref(), content)
    }

    /// The clean `Pass` verdict emitted when a file matches a skip pattern.
    fn skip_verdict(start: Instant) -> Verdict {
        Verdict {
            schema_version: crate::verdict::SCHEMA_VERSION,
            hector_version: env!("CARGO_PKG_VERSION").to_string(),
            status: Status::Pass,
            violations: vec![],
            passed_checks: vec![],
            elapsed_ms: start.elapsed().as_millis() as u64,
            deferred_rules: vec![],
        }
    }

    /// Append one `Check` line to the telemetry log. Best-effort: a failed
    /// append (disk-full, unwritable path, FS lock) warns to stderr but
    /// never fails the check — the log is never the source of truth.
    fn append_check_log(
        &self,
        file: &str,
        status: Status,
        elapsed_ms: u64,
        rules: Vec<PerRuleRecord>,
    ) {
        if let Err(e) = crate::telemetry::append(
            &self.config_dir.join(".hector/log.jsonl"),
            &LogEntry::Check {
                ts: chrono::Utc::now().to_rfc3339(),
                file: file.to_string(),
                status,
                elapsed_ms,
                rules,
            },
        ) {
            eprintln!("hector: telemetry append failed: {e:#}");
        }
    }

    /// Split the resolved rules into those dispatched through the engine
    /// pool and those deferred to an in-session evaluator.
    ///
    /// The `--rule` filter is applied here, upstream of dispatch, so a
    /// filtered-out rule never enters the work queue or triggers its engine
    /// (in particular, no LLM call). With `emit_semantic_payload` set, the
    /// semantic/session rules are collected into the deferred set instead of
    /// dispatched; deterministic rules still run. Deferred rules out of
    /// scope for the file are dropped, mirroring the dispatch path.
    fn partition_rules(&self, match_path: &Path, collect_explain: bool) -> RulePartition<'_> {
        let filter: &HashSet<String> = &self.options.rules;
        let mut partition = RulePartition {
            selected: Vec::new(),
            deferred: Vec::new(),
            explain: Vec::new(),
        };
        for (rule_id, rule) in &self.config.rules {
            if !filter.is_empty() && !filter.contains(rule_id.as_str()) {
                continue;
            }
            if !should_defer(rule.engine, &self.options) {
                partition.selected.push((rule_id, rule));
                continue;
            }
            if !self.rule_matches_path(rule_id, match_path) {
                continue;
            }
            partition
                .deferred
                .push(crate::verdict_deferred::DeferredRule {
                    id: rule_id.clone(),
                    description: rule.description.clone(),
                    severity: severity_string(rule.severity),
                    engine: match rule.engine {
                        EngineKind::Semantic => "semantic".into(),
                        EngineKind::Session => "session".into(),
                        _ => unreachable!("should_defer guards on Semantic/Session"),
                    },
                });
            if collect_explain {
                partition.explain.push(RuleExplain {
                    rule_id: rule_id.clone(),
                    engine: rule.engine,
                    outcome: ExplainOutcome::Skipped {
                        reason: "deferred_subagent".into(),
                    },
                });
            }
        }
        partition
    }

    /// Evaluate the selected rules and fold their outcomes. Output order
    /// matches input (`BTreeMap` key order), so the parallel collect is
    /// deterministic. Single-rule workloads skip pool construction.
    fn dispatch_selected(
        &self,
        selected: &[(&String, &Rule)],
        inputs: &CheckInputs<'_>,
    ) -> DispatchOutcome {
        let outcomes: Vec<RuleOutcome> = if selected.len() <= 1 {
            selected
                .iter()
                .map(|(rule_id, rule)| self.evaluate_one_rule(rule_id, rule, inputs))
                .collect()
        } else {
            let pool = self.execution_pool();
            pool.install(|| {
                selected
                    .par_iter()
                    .map(|(rule_id, rule)| self.evaluate_one_rule(rule_id, rule, inputs))
                    .collect()
            })
        };

        let mut out = DispatchOutcome::default();
        for outcome in outcomes {
            out.violations.extend(outcome.violations);
            out.passed.extend(outcome.passed);
            out.explain.extend(outcome.explain);
            out.records.extend(outcome.record);
        }
        out
    }

    /// Drop violations already recorded in the baseline. A corrupt or
    /// unreadable baseline warns and is treated as empty so the check still
    /// runs; a missing baseline (the common first-run state) is silent.
    /// `content` lets the baseline compare each stored `line_sha256` against
    /// the current line text.
    fn apply_baseline(&self, violations: &mut Vec<Violation>, content: &str) {
        let baseline_path = self.config_dir.join(".hector/baseline.json");
        let baseline = match crate::baseline::Baseline::load(&baseline_path) {
            Ok(b) => b,
            Err(e) => {
                let is_missing = e
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
                if !is_missing {
                    eprintln!(
                        "hector: warning — baseline at {} is corrupt or unreadable: {e:#}; ignoring",
                        baseline_path.display()
                    );
                }
                crate::baseline::Baseline::default()
            }
        };
        violations.retain(|v| !baseline.contains_with_content(v, Some(content)));
    }

    /// Pre-expand the deferred rules' evaluation contexts and build the
    /// evaluator-input string the in-session subagent and the direct-API
    /// path both consume — keeping the evidence byte-identical between them.
    ///
    /// Expansion failures are pushed onto `violations` as `__internal`
    /// entries (yielding an `InternalError` verdict, matching the direct-API
    /// path) rather than silently dropped. Returns `None` when no context
    /// expanded, so an all-failures run reads as "no input" rather than an
    /// empty string.
    fn build_deferred_evaluator_input(
        &self,
        deferred_rules: &[crate::verdict_deferred::DeferredRule],
        path: &Path,
        diff: &str,
        content: Option<&str>,
        violations: &mut Vec<Violation>,
    ) -> Option<String> {
        let expansion = self.expand_deferred_contexts(deferred_rules, path, diff, content);
        push_expansion_failures_into_violations(&expansion.failures, path, violations);
        if expansion.successes.is_empty() {
            return None;
        }
        let sentinel = crate::llm::prompt::Sentinel::new_random();
        Some(crate::llm::prompt::build_evaluator_input(
            &expansion.successes,
            &sentinel,
        ))
    }

    /// When a deterministic block fires during an `emit_semantic_payload`
    /// run, the CLI suppresses the deferred envelope and prints only the
    /// block verdict — which would erase the deferred rules. Surface them on
    /// `verdict.deferred_rules` so the operator still sees what their policy
    /// would have evaluated. No-op unless the verdict is a block with
    /// deferred rules present.
    fn surface_deferred_on_block(
        &self,
        verdict: &mut Verdict,
        deferred_rules: &[crate::verdict_deferred::DeferredRule],
    ) {
        if !matches!(verdict.status, Status::Block) || deferred_rules.is_empty() {
            return;
        }
        verdict.deferred_rules = deferred_rules
            .iter()
            .filter_map(|d| {
                self.config_rule(&d.id)
                    .map(|r| crate::verdict::DeferredRuleRef {
                        rule_id: d.id.clone(),
                        severity: match r.severity {
                            crate::config::Severity::Error => crate::verdict::Severity::Error,
                            crate::config::Severity::Warning => crate::verdict::Severity::Warning,
                        },
                        reason: "suppressed by deterministic block".to_string(),
                    })
            })
            .collect();
    }

    /// Central orchestration: normalize the input, short-circuit skipped
    /// files, partition rules, dispatch the deterministic ones, filter the
    /// baseline, build the deferred envelope, and log telemetry.
    fn check_inner(&self, input: CheckInput, collect_explain: bool) -> Result<CheckReport> {
        use crate::disable::DisableMap;
        let start = Instant::now();

        let (path, content, diff, content_authoritative) =
            match self.resolve_check_input(input, start) {
                InputResolution::Resolved {
                    path,
                    content,
                    diff,
                    content_authoritative,
                } => (path, content, diff, content_authoritative),
                InputResolution::Rejected(verdict) => {
                    return Ok(CheckReport {
                        verdict,
                        explain: vec![],
                        deferred: None,
                    });
                }
            };

        if self.skip.matches(&path) {
            let verdict = Self::skip_verdict(start);
            self.append_check_log(
                &path.display().to_string(),
                verdict.status,
                verdict.elapsed_ms,
                vec![],
            );
            return Ok(CheckReport {
                verdict,
                explain: Vec::new(),
                deferred: None,
            });
        }

        let disable_map = DisableMap::from_source(&content);
        let match_path = relativize(&path, &self.config_dir);
        let inputs = CheckInputs {
            match_path: &match_path,
            path: &path,
            // Authoritative content passes through verbatim (empty proposed
            // content is still a valid target). Non-authoritative empty
            // content collapses to `None` so engines emit `__internal`
            // rather than passing on a missed read.
            content: if content_authoritative || !content.is_empty() {
                Some(content.as_str())
            } else {
                None
            },
            diff: &diff,
            disable_map: &disable_map,
            collect_explain,
        };

        let RulePartition {
            selected,
            deferred,
            mut explain,
        } = self.partition_rules(&match_path, collect_explain);

        let mut dispatch = self.dispatch_selected(&selected, &inputs);
        explain.append(&mut dispatch.explain);

        self.apply_baseline(&mut dispatch.violations, &content);

        // Computed before `deferred` is consumed by `build_deferred_envelope`
        // — the expansion tuples borrow each `DeferredRule`.
        let evaluator_input = self.build_deferred_evaluator_input(
            &deferred,
            &path,
            &diff,
            inputs.content,
            &mut dispatch.violations,
        );

        let mut verdict = Verdict::from_violations(
            dispatch.violations,
            dispatch.passed,
            start.elapsed().as_millis() as u64,
        );
        self.surface_deferred_on_block(&mut verdict, &deferred);

        self.append_check_log(
            &path.display().to_string(),
            verdict.status,
            verdict.elapsed_ms,
            dispatch.records,
        );

        let deferred =
            self.build_deferred_envelope(deferred, &path, &diff, &verdict, evaluator_input);
        Ok(CheckReport {
            verdict,
            explain,
            deferred,
        })
    }

    /// Assemble the `DeferredVerdict` envelope from the rules
    /// `should_defer` short-circuited; `None` when there are none. The
    /// envelope sweeps warn-severity violations onto `payload.warnings`
    /// (the CLI suppresses the verdict JSON when it emits an envelope),
    /// threads `expand_context` per rule so the subagent and direct-API
    /// routes read the same prompt, and rolls a fresh sentinel each time.
    fn build_deferred_envelope(
        &self,
        deferred_rules: Vec<crate::verdict_deferred::DeferredRule>,
        path: &Path,
        diff: &str,
        verdict: &Verdict,
        evaluator_input: Option<String>,
    ) -> Option<crate::verdict_deferred::DeferredVerdict> {
        if deferred_rules.is_empty() {
            return None;
        }
        // Suppress the envelope on terminal verdict states. The CLI gates on
        // this too, but pinning it here means library callers get the same
        // contract: no envelope once the verdict says "stop." On Block, the
        // deferred rules are surfaced on `verdict.deferred_rules` instead; on
        // InternalError, an engine failure has short-circuited the dispatch
        // the envelope exists to enable.
        if matches!(verdict.status, Status::Block | Status::InternalError) {
            return None;
        }
        let evaluator_input = evaluator_input.unwrap_or_default();

        // Thread the optional evaluator_model override from the loaded `llm:`
        // block into the payload. Only the subagent provider reads it.
        let evaluator_model = self
            .config
            .llm
            .as_ref()
            .and_then(|l| l.evaluator_model.clone());

        let warnings = build_deferred_warnings(verdict);

        Some(crate::verdict_deferred::DeferredVerdict {
            schema_version: crate::verdict_deferred::DEFERRED_SCHEMA_VERSION,
            deferred: true,
            hector_version: env!("CARGO_PKG_VERSION").to_string(),
            passed_checks: verdict.passed_checks.clone(),
            payload: crate::verdict_deferred::DeferredPayload {
                file: path.display().to_string(),
                diff: diff.to_string(),
                passed_checks: verdict.passed_checks.clone(),
                evaluate: deferred_rules,
                evaluator_input,
                evaluator_model,
                warnings,
            },
            elapsed_ms: verdict.elapsed_ms,
        })
    }

    /// Expand each deferred rule's context via the same
    /// `engine::context::expand_context` that `render_semantic_prompts` uses,
    /// so the envelope's `evaluator_input` and the direct-API prompt produce
    /// byte-identical evidence for the same `(rule, input)`.
    ///
    /// Expansion errors are returned in `failures` (not dropped) so the
    /// caller can thread them into violations as `__internal` entries before
    /// the verdict is finalised, matching the direct-API path.
    ///
    /// The `'a` constraint ties the returned tuples to `deferred_rules` and
    /// `self`: `RuleRef::id` borrows the `DeferredRule` slice and
    /// `RuleRef::rule` borrows the config map.
    fn expand_deferred_contexts<'a>(
        &'a self,
        deferred_rules: &'a [crate::verdict_deferred::DeferredRule],
        path: &Path,
        diff: &str,
        content: Option<&str>,
    ) -> DeferredExpansion<'a> {
        let mut successes = Vec::with_capacity(deferred_rules.len());
        let mut failures: Vec<(String, anyhow::Error)> = Vec::new();
        for d in deferred_rules {
            let Some(rule) = self.config_rule(&d.id) else {
                continue;
            };
            let scope = rule.context.unwrap_or(crate::config::ContextScope::Diff);
            let expansion = crate::engine::context::expand_context(
                scope,
                if diff.is_empty() { None } else { Some(diff) },
                Some(path),
                content,
                &self.config_dir,
            );
            match expansion {
                Ok((primary, context_text)) => {
                    successes.push((
                        crate::llm::prompt::RuleRef { id: &d.id, rule },
                        primary,
                        context_text,
                    ));
                }
                Err(e) => {
                    failures.push((d.id.clone(), e));
                }
            }
        }
        DeferredExpansion {
            successes,
            failures,
        }
    }

    /// Render the prompts that *would* be sent for every in-scope semantic
    /// rule, without dispatching — `hector check --print-prompt` uses this to
    /// debug prompt construction without burning API calls. Honors the
    /// `--rule` filter and per-rule scope; non-semantic rules are skipped.
    pub fn render_semantic_prompts(&self, input: CheckInput) -> Result<Vec<RenderedPrompt>> {
        let (path, diff, content) = match input {
            CheckInput::File { path, content } => {
                let resolved = self.resolve_input_path(&path)?;
                let diff = self.synthesize_file_diff(&resolved, &content);
                (resolved, diff, Some(content))
            }
            CheckInput::Diff { file, unified_diff } => (file, unified_diff, None),
        };
        let match_path = relativize(&path, &self.config_dir);
        let mut out = Vec::new();
        for (rule_id, rule) in &self.config.rules {
            if !self.options.rules.is_empty() && !self.options.rules.contains(rule_id) {
                continue;
            }
            if rule.engine != EngineKind::Semantic {
                continue;
            }
            // Use the load-time cached matcher — no GlobSet rebuild per call.
            if !self.rule_matches_path(rule_id, &match_path) {
                continue;
            }
            let scope = rule.context.unwrap_or(crate::config::ContextScope::Diff);
            let (primary, context_text) = crate::engine::context::expand_context(
                scope,
                if diff.is_empty() { None } else { Some(&diff) },
                Some(&path),
                content.as_deref(),
                &self.config_dir,
            )?;
            let (system, user) = crate::llm::prompt::build_prompt_split(
                &[(rule_id.as_str(), rule)],
                &primary,
                context_text.as_deref(),
            );
            out.push(RenderedPrompt {
                rule_id: rule_id.clone(),
                system,
                user,
            });
        }
        Ok(out)
    }

    /// If the rule is semantic and the diff cannot plausibly match it,
    /// record a `semantic_skipped` telemetry entry and return
    /// `Some(reason)` so the caller skips engine dispatch (the same
    /// reason string also feeds the C4 `--explain` row). Otherwise
    /// return `None`.
    ///
    /// Only applies in diff mode: `CheckInput::File` passes an empty
    /// `diff` here, which `can_match_diff` would classify as
    /// `SkipReason::Empty` — bypassing every file-mode semantic rule
    /// silently, which is incorrect. The empty-diff guard preserves
    /// file-mode semantics: there is no diff to analyze, so dispatch.
    ///
    /// The pre-filter lives in the runner (not inside `SemanticEngine`)
    /// so it sits alongside the other cross-cutting concerns (scope,
    /// baseline, disable, skip) and the engine stays pure — no HTTP
    /// request leaves the engine when this fires.
    fn try_semantic_skip(
        &self,
        rule_id: &str,
        rule: &Rule,
        path: &Path,
        diff: &str,
    ) -> Option<String> {
        if rule.engine != EngineKind::Semantic || diff.is_empty() {
            return None;
        }
        let analysis = crate::diff::analysis::can_match_diff(diff, path, &rule.description);
        let crate::diff::analysis::CanMatch::No(reason) = analysis else {
            return None;
        };
        let reason_str = reason.as_str().to_string();
        let log_path = self.config_dir.join(".hector/log.jsonl");
        let entry = LogEntry::SemanticSkipped {
            ts: chrono::Utc::now().to_rfc3339(),
            file: path.display().to_string(),
            rule: rule_id.to_string(),
            reason: reason_str.clone(),
        };
        if let Err(e) = crate::telemetry::append(&log_path, &entry) {
            eprintln!("hector: telemetry append failed: {e:#}");
        }
        Some(reason_str)
    }

    /// Fold one session rule's outcome into the shared accumulators and emit
    /// a `SemanticVerdict` for each rule that reached the LLM.
    fn absorb_session_outcome(
        &self,
        rule_id: &str,
        rule: &Rule,
        evaluation: Result<Option<Violation>>,
        elapsed: u64,
        acc: &mut SessionAcc<'_>,
    ) {
        match evaluation {
            Ok(Some(v)) => {
                acc.violations.push(v);
                self.append_semantic_verdict(rule_id, None, "violation");
                let status = match rule.severity {
                    crate::config::Severity::Error => Status::Block,
                    crate::config::Severity::Warning => Status::Warn,
                };
                acc.records.push(PerRuleRecord {
                    rule_id: rule_id.to_string(),
                    engine: crate::verdict::Engine::Session,
                    status,
                    elapsed_ms: elapsed,
                    reason: None,
                });
            }
            Ok(None) => {
                acc.passed.push(rule_id.to_string());
                self.append_semantic_verdict(rule_id, None, "pass");
                acc.records.push(PerRuleRecord {
                    rule_id: rule_id.to_string(),
                    engine: crate::verdict::Engine::Session,
                    status: Status::Pass,
                    elapsed_ms: elapsed,
                    reason: None,
                });
            }
            // Session-engine runtime errors are Engine::Internal.
            Err(e) => {
                acc.violations.push(crate::verdict::Violation {
                    rule_id: format!("{rule_id}__internal"),
                    severity: crate::verdict::Severity::Error,
                    engine: crate::verdict::Engine::Internal,
                    file: "".to_string(),
                    line: None,
                    column: None,
                    message: format!("{e:#}"),
                    suggestion: None,
                    context: None,
                });
                acc.records.push(PerRuleRecord {
                    rule_id: rule_id.to_string(),
                    engine: crate::verdict::Engine::Session,
                    status: Status::Block,
                    elapsed_ms: elapsed,
                    reason: Some("engine_error".into()),
                });
            }
        }
    }

    pub fn check_session(
        &self,
        state: &crate::session_state::SessionState,
    ) -> Result<crate::verdict::Verdict> {
        use crate::engine::session::SessionEngine;

        let start = Instant::now();
        let mut violations = Vec::new();
        let mut passed = Vec::new();
        let mut records: Vec<PerRuleRecord> = Vec::new();
        let session_engine = SessionEngine;
        for (rule_id, rule) in &self.config.rules {
            if rule.engine != crate::config::EngineKind::Session {
                continue;
            }
            // Per-edit scope filtering. The session engine aggregates
            // `state.edits` into one LLM prompt; without filtering, a rule
            // scoped to `src/auth/**` would fire on a session whose every edit
            // lives under `src/billing/`. `rule_matches_path` (as in
            // `check_inner`) relativizes absolute adapter paths before
            // matching, so pathed scopes fire even when edits carry absolute
            // paths.
            let filtered_edits: Vec<crate::session_state::EditRecord> = state
                .edits
                .iter()
                .filter(|e| self.rule_matches_path(rule_id, std::path::Path::new(&e.file)))
                .cloned()
                .collect();
            if filtered_edits.is_empty() {
                passed.push(rule_id.clone());
                records.push(PerRuleRecord {
                    rule_id: rule_id.clone(),
                    engine: crate::verdict::Engine::Session,
                    status: Status::Pass,
                    elapsed_ms: 0,
                    reason: None,
                });
                continue;
            }
            let scoped_state = crate::session_state::SessionState {
                session_id: state.session_id.clone(),
                started_at: state.started_at.clone(),
                edits: filtered_edits,
            };
            let llm = self.llm.as_deref().ok_or_else(|| {
                anyhow::anyhow!("session check requires LlmClient; build engine with .with_llm()")
            })?;
            let rule_start = Instant::now();
            let evaluation = session_engine.evaluate(&scoped_state, rule_id, rule, llm);
            let rule_elapsed = rule_start.elapsed().as_millis() as u64;
            let mut acc = SessionAcc {
                violations: &mut violations,
                passed: &mut passed,
                records: &mut records,
            };
            self.absorb_session_outcome(rule_id, rule, evaluation, rule_elapsed, &mut acc);
        }
        let verdict = crate::verdict::Verdict::from_violations(
            violations,
            passed,
            start.elapsed().as_millis() as u64,
        );
        // Best-effort, same as the per-file append above.
        if let Err(e) = crate::telemetry::append(
            &self.config_dir.join(".hector/log.jsonl"),
            &LogEntry::Check {
                ts: chrono::Utc::now().to_rfc3339(),
                file: "".into(),
                status: verdict.status,
                elapsed_ms: verdict.elapsed_ms,
                rules: records,
            },
        ) {
            eprintln!("hector: telemetry append failed: {e:#}");
        }
        Ok(verdict)
    }

    /// Session-stop path for the Claude Code subagent provider.
    ///
    /// When `options.emit_semantic_payload` is true AND at least one
    /// `engine: session` rule is in scope for at least one edit, this
    /// method emits a [`CheckReport`] whose `deferred` field carries a
    /// [`crate::verdict_deferred::DeferredVerdict`] with:
    /// - `file: ""` (session-level, not per-file)
    /// - `diff: <framed aggregate>` (every in-scope edit framed via
    ///   `engine::session::framed_aggregate`)
    ///
    /// When no session rule is in scope, or `emit_semantic_payload` is
    /// false, falls through to `check_session` and wraps the result in
    /// a `CheckReport` with `deferred: None`.
    pub fn check_session_with_options(
        &self,
        state: &crate::session_state::SessionState,
    ) -> Result<CheckReport> {
        use crate::engine::session::framed_aggregate;

        // In deferred mode, collect in-scope session rules into an envelope
        // instead of requiring an LlmClient.
        if self.options.emit_semantic_payload {
            let start = Instant::now();
            let mut deferred_rules: Vec<crate::verdict_deferred::DeferredRule> = Vec::new();
            let mut passed: Vec<String> = Vec::new();
            let filter: &HashSet<String> = &self.options.rules;

            for (rule_id, rule) in &self.config.rules {
                if rule.engine != crate::config::EngineKind::Session {
                    continue;
                }
                if !filter.is_empty() && !filter.contains(rule_id.as_str()) {
                    continue;
                }
                // Per-edit scope filter: the rule must match at least one
                // edit's file path to be considered in scope.
                let any_in_scope = state
                    .edits
                    .iter()
                    .any(|e| self.rule_matches_path(rule_id, std::path::Path::new(&e.file)));
                if !any_in_scope {
                    passed.push(rule_id.clone());
                    continue;
                }
                deferred_rules.push(crate::verdict_deferred::DeferredRule {
                    id: rule_id.clone(),
                    description: rule.description.clone(),
                    severity: severity_string(rule.severity),
                    engine: "session".into(),
                });
            }

            if !deferred_rules.is_empty() {
                // payload.diff is the FULL session aggregate (every edit) —
                // surfaced to the operator and the in-session subagent as
                // session-level meta-context. The per-rule evaluator slice
                // below is what the subagent actually evaluates with.
                let aggregate_diff = framed_aggregate(state);

                // Build the per-rule evaluator input. Each session rule sees
                // only the edits matching its own scope — the same per-rule
                // scoping `check_session` applies before calling
                // `framed_aggregate`, which the "direct-API and subagent see
                // the same evidence" invariant requires.
                let sentinel = crate::llm::prompt::Sentinel::new_random();
                let evaluator_tuples: Vec<(
                    crate::llm::prompt::RuleRef<'_>,
                    String,
                    Option<String>,
                )> = deferred_rules
                    .iter()
                    .filter_map(|d| {
                        let rule = self.config_rule(&d.id)?;
                        let scoped_state = crate::session_state::SessionState {
                            session_id: state.session_id.clone(),
                            started_at: state.started_at.clone(),
                            edits: state
                                .edits
                                .iter()
                                .filter(|e| {
                                    self.rule_matches_path(&d.id, std::path::Path::new(&e.file))
                                })
                                .cloned()
                                .collect(),
                        };
                        let scoped_diff = framed_aggregate(&scoped_state);
                        Some((
                            crate::llm::prompt::RuleRef { id: &d.id, rule },
                            scoped_diff,
                            None,
                        ))
                    })
                    .collect();
                let evaluator_input =
                    crate::llm::prompt::build_evaluator_input(&evaluator_tuples, &sentinel);

                let evaluator_model = self
                    .config
                    .llm
                    .as_ref()
                    .and_then(|l| l.evaluator_model.clone());

                let verdict = crate::verdict::Verdict::from_violations(
                    vec![],
                    passed,
                    start.elapsed().as_millis() as u64,
                );

                let deferred = Some(crate::verdict_deferred::DeferredVerdict {
                    schema_version: crate::verdict_deferred::DEFERRED_SCHEMA_VERSION,
                    deferred: true,
                    hector_version: env!("CARGO_PKG_VERSION").to_string(),
                    passed_checks: verdict.passed_checks.clone(),
                    payload: crate::verdict_deferred::DeferredPayload {
                        file: "".to_string(),
                        diff: aggregate_diff,
                        passed_checks: verdict.passed_checks.clone(),
                        evaluate: deferred_rules,
                        evaluator_input,
                        evaluator_model,
                        warnings: vec![],
                    },
                    elapsed_ms: verdict.elapsed_ms,
                });

                return Ok(CheckReport {
                    verdict,
                    explain: vec![],
                    deferred,
                });
            }
        }

        // Fallback: no deferred session rules in scope (or not in deferred
        // mode). Delegate to the existing LLM-dispatch path.
        let verdict = self.check_session(state)?;
        Ok(CheckReport {
            verdict,
            explain: vec![],
            deferred: None,
        })
    }
}
