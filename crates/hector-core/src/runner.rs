use crate::config::skip::{parse_user_global_ignore, SkipMatcher, USER_GLOBAL_IGNORE_FILENAME};
use crate::config::{Config, EngineKind, Rule};
use crate::engine::{RuleContext, RuleEngine};
use crate::telemetry::{LogEntry, PerRuleRecord};
use crate::verdict::{Status, Verdict, Violation};
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Map a config `EngineKind` to the verdict-side `Engine` used in
/// telemetry. Free-standing helper so the per-rule record construction
/// stays a single-line expression in the hot paths.
fn engine_kind_to_verdict_engine(kind: EngineKind) -> crate::verdict::Engine {
    match kind {
        EngineKind::Script => crate::verdict::Engine::Script,
        EngineKind::Ast => crate::verdict::Engine::Ast,
        EngineKind::Semantic => crate::verdict::Engine::Semantic,
        EngineKind::Session => crate::verdict::Engine::Session,
    }
}

/// H1: decide whether a semantic or session rule should be collected
/// into the deferred envelope instead of dispatched. Returns true only
/// when the option is set AND the engine is one of the two LLM-dispatch
/// engines — `Script` and `Ast` always run.
fn should_defer(engine: EngineKind, options: &CheckOptions) -> bool {
    options.emit_semantic_payload && matches!(engine, EngineKind::Semantic | EngineKind::Session)
}

/// H1: render a `Severity` as the bully-compatible string the deferred
/// envelope's `severity` field carries.
fn severity_string(s: crate::config::Severity) -> String {
    match s {
        crate::config::Severity::Error => "error".into(),
        crate::config::Severity::Warning => "warning".into(),
    }
}

/// C4: optional per-run knobs for `HectorEngine::check`. Plumbed via
/// `HectorEngine::builder().with_options(...)` so the public `check`
/// signature stays stable across additions.
#[derive(Debug, Clone, Default)]
pub struct CheckOptions {
    /// Restrict evaluation to these rule ids. Empty set = run all rules.
    /// The runner enforces the filter *upstream* of the parallel
    /// dispatch pool, so filtered-out rules never enter the work queue
    /// and never trigger their engine (in particular, no LLM call).
    pub rules: HashSet<String>,
    /// If true, capture per-rule outcomes for the explain report.
    pub explain: bool,
    /// H1: when true, `engine: semantic` and `engine: session` rules are
    /// not dispatched — they are collected into [`CheckReport::deferred`]
    /// for an in-session Claude Code subagent to evaluate.
    pub emit_semantic_payload: bool,
}

/// C4: one row of the `--explain` report. Stays out of the verdict JSON
/// (verdict shape is locked at 0.1) — surfaced to the CLI via
/// [`CheckReport`].
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
    /// Rule was short-circuited before engine dispatch (e.g. A3 diff
    /// pre-filter) or the engine returned an error.
    Skipped { reason: String },
}

/// C4: companion return shape for [`HectorEngine::check_with_explain`].
#[derive(Debug, Clone)]
pub struct CheckReport {
    pub verdict: Verdict,
    pub explain: Vec<RuleExplain>,
    /// H1: present when `CheckOptions::emit_semantic_payload` was true
    /// and at least one semantic/session rule survived scope/skip/
    /// diff-prefilter. `None` otherwise. The CLI inspects this to
    /// decide whether to emit a `DeferredVerdict` or a standard
    /// `Verdict`.
    pub deferred: Option<crate::verdict_deferred::DeferredVerdict>,
}

/// C4: one rendered semantic prompt. `system` + `user` mirror Anthropic's
/// `/v1/messages` split; OpenAI-compat providers concatenate them.
#[derive(Debug, Clone)]
pub struct RenderedPrompt {
    pub rule_id: String,
    pub system: String,
    pub user: String,
}

/// C2: snapshot of which rules are in scope for a given file, plus any
/// skip-pattern hit. Returned by [`HectorEngine::scope_outcomes`] and
/// rendered by `hector explain` / `hector guide` in the CLI.
///
/// This is the *read-only* counterpart to `check_inner`'s scope walk. No
/// engine runs, no LLM is constructed, no telemetry is written.
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

/// C2: which skip pattern (built-in or user-supplied) matched the file.
///
/// `pattern` is the *raw* glob string the matcher was built from — what
/// the author would put in `skip:` to reproduce or override the hit.
#[derive(Debug, Clone)]
pub struct SkipHit {
    pub pattern: String,
}

/// C2: per-rule scope outcome.
///
/// `engine`, `severity`, and `description` are mirrored here (cheap
/// clones of `Copy` enums + a `String`) so `guide` can render its
/// `<rule-id> [<severity>] <description>` line without re-borrowing
/// the engine — that lets the helper be called once and the result
/// rendered out into either format.
#[derive(Debug, Clone)]
pub struct RuleScopeEntry {
    pub rule_id: String,
    pub engine: EngineKind,
    pub severity: crate::config::Severity,
    pub description: String,
    pub scope_match: ScopeMatch,
}

/// C2: scope-match outcome for one rule against one file.
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
///
/// C4: `explain` carries an optional explain row. The row is produced
/// inside `evaluate_one_rule` so the parallel dispatch keeps a single
/// per-rule output type — the runner concatenates explain rows after
/// collecting outcomes. When `CheckInputs.collect_explain` is false the
/// field is always `None` (one branch, zero allocation).
struct RuleOutcome {
    violations: Vec<Violation>,
    passed: Option<String>,
    explain: Option<RuleExplain>,
    /// D1: per-rule telemetry line. Always populated when the rule
    /// reached engine dispatch (or was short-circuited by A3); `None`
    /// when the rule was out-of-scope (won't appear in the Check.rules
    /// array, matches "rule didn't run for this file" semantics).
    record: Option<PerRuleRecord>,
}

/// D1: per-call accumulators for `check_session`. Bundled into a single
/// struct so `absorb_session_outcome` stays under the workspace's
/// argument-count lint while still owning the three independent vecs.
struct SessionAcc<'a> {
    violations: &'a mut Vec<Violation>,
    passed: &'a mut Vec<String>,
    records: &'a mut Vec<PerRuleRecord>,
}

/// Per-file inputs reused across every rule evaluation in one `check()`
/// call. Bundled into a single struct so `evaluate_one_rule` stays under
/// the workspace's argument-count lint.
struct CheckInputs<'a> {
    match_path: &'a Path,
    path: &'a Path,
    content: &'a str,
    diff: &'a str,
    disable_map: &'a crate::disable::DisableMap,
    /// C4: build a `RuleExplain` row for every rule whose evaluation
    /// reaches engine dispatch (or is short-circuited by the A3 diff
    /// pre-filter). Out-of-scope and filter-skipped rules never enter
    /// `evaluate_one_rule` so they don't appear in the report.
    collect_explain: bool,
}

pub struct HectorEngine {
    config: Config,
    config_dir: PathBuf,
    llm: Option<Box<dyn crate::llm::LlmClient>>,
    skip: SkipMatcher,
    options: CheckOptions,
}

/// Resolve the current user's home directory from environment variables.
/// Mirrors what `dirs::home_dir` does on Unix and Windows without the dep.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

/// Resolve `path` to a form that can be matched against a relative scope glob
/// authored in `config_dir`-relative terms.
///
/// Two fallback layers:
/// 1. `canonicalize` failure (file missing — e.g. diff mode references a path
///    not yet on disk) returns the original `PathBuf` so the scope match can
///    still proceed against the literal input.
/// 2. `strip_prefix` failure (the input resolves outside `config_dir` — e.g.
///    `hector check /etc/passwd` against a `~/proj/.hector.yml`) returns the
///    canonicalized absolute path. Bare-pattern globs in `config/scope.rs`
///    register a `**/<pattern>` form, so absolute paths can still match
///    rules like `*.py` via that fallback.
fn relativize(path: &std::path::Path, root: &std::path::Path) -> std::path::PathBuf {
    let canon_path = path.canonicalize().unwrap_or_else(|_| PathBuf::from(path));
    let canon_root = root.canonicalize().unwrap_or_else(|_| PathBuf::from(root));
    canon_path
        .strip_prefix(&canon_root)
        .map(PathBuf::from)
        .unwrap_or(canon_path)
}

/// C2: identify which raw skip glob matched a path. Mirrors the
/// construction order in `SkipMatcher::with_built_ins` (built-ins first,
/// user extras second) so the reported pattern matches what the author
/// would type to reproduce the skip. Returns `None` when no pattern
/// matches — the caller should treat that as "file is in scope for the
/// usual rule walk." Silently returns `None` on any glob construction
/// error, since the same globs already round-tripped through
/// `SkipMatcher::with_built_ins` at engine load time.
fn first_matching_skip_glob(file: &std::path::Path, extras: &[String]) -> Option<String> {
    use globset::{Glob, GlobSetBuilder};
    let candidates: Vec<String> = crate::config::skip::built_in_skip_globs()
        .iter()
        .map(|s| (*s).to_string())
        .chain(extras.iter().cloned())
        .collect();
    for raw in candidates {
        let mut b = GlobSetBuilder::new();
        let Ok(g) = Glob::new(&raw) else {
            continue;
        };
        b.add(g);
        if !raw.contains('/') {
            let Ok(g2) = Glob::new(&format!("**/{raw}")) else {
                continue;
            };
            b.add(g2);
        } else if let Some(prefix) = raw.strip_suffix("/**") {
            if !prefix.is_empty() && !prefix.contains('*') {
                let Ok(g3) = Glob::new(&format!("**/{prefix}/**")) else {
                    continue;
                };
                b.add(g3);
            }
        }
        let Ok(set) = b.build() else { continue };
        if set.is_match(file) {
            return Some(raw);
        }
    }
    None
}

/// C2: walk a rule's scope list in author order and return the first
/// glob that matches `path`. Returns `None` if no glob matches. Mirrors
/// the right-anchored bare-pattern semantics of
/// `crate::config::scope::ScopeMatcher` (a bare `*.py` also matches at
/// any depth via the `**/<pattern>` form).
fn first_matching_scope_glob(scopes: &[String], path: &std::path::Path) -> Option<String> {
    use globset::{Glob, GlobSetBuilder};
    for raw in scopes {
        let mut b = GlobSetBuilder::new();
        let Ok(g) = Glob::new(raw) else { continue };
        b.add(g);
        if !raw.contains('/') {
            let Ok(g2) = Glob::new(&format!("**/{raw}")) else {
                continue;
            };
            b.add(g2);
        }
        let Ok(set) = b.build() else { continue };
        if set.is_match(path) {
            return Some(raw.clone());
        }
    }
    None
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

    /// C4: attach optional per-run knobs (rule filter, explain capture).
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

/// C4: translate `(engine, errored, emitted)` into the explain outcome for
/// a rule that *did* reach engine dispatch (i.e. wasn't filtered or
/// short-circuited by the A3 diff pre-filter — those produce their own
/// rows upstream).
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

    /// C4: iterator over every rule id in the loaded config. Used by the
    /// CLI to validate `--rule` arguments at the boundary, before any
    /// dispatch happens.
    pub fn config_rule_ids(&self) -> impl Iterator<Item = &str> {
        self.config.rules.keys().map(|k| k.as_str())
    }

    /// H1: lookup a rule by id from the loaded config. Used to resolve
    /// `DeferredRule` ids back to their full definitions when building
    /// the evaluator-input string.
    pub(crate) fn config_rule(&self, id: &str) -> Option<&crate::config::Rule> {
        self.config.rules.get(id)
    }

    /// C2: read-only scope walk. Returns the skip-pattern hit (if any)
    /// and a per-rule scope outcome for every rule in the resolved config.
    /// No engine runs; no LLM is constructed; no telemetry is written.
    ///
    /// Used by `hector explain <file>` and `hector guide <file>` so they
    /// share one source of truth for "what's in scope for this path?"
    /// with `hector check`'s dispatch loop. The path is relativized
    /// against the config dir using the same fallback rules as the
    /// regular check path, so an absolute `/etc/passwd` and a relative
    /// `etc/passwd` produce the same per-rule outcome shape.
    pub fn scope_outcomes(&self, file: &std::path::Path) -> ScopeOutcomes {
        let match_path = relativize(file, &self.config_dir);

        // Skip resolution. Mirror `load_with`'s extras assembly so the
        // helper sees the same union of project + user-global globs.
        let mut extras = self.config.skip.clone();
        if let Some(home) = home_dir() {
            let ignore_path = home.join(USER_GLOBAL_IGNORE_FILENAME);
            if let Ok(raw) = std::fs::read_to_string(&ignore_path) {
                extras.extend(parse_user_global_ignore(&raw));
            }
        }
        let skip =
            first_matching_skip_glob(&match_path, &extras).map(|pattern| SkipHit { pattern });

        let mut rules: Vec<RuleScopeEntry> = Vec::with_capacity(self.config.rules.len());
        for (rule_id, rule) in &self.config.rules {
            let matched = first_matching_scope_glob(&rule.scope, &match_path);
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

    fn load_with(
        config_path: &Path,
        llm_override: Option<Box<dyn crate::llm::LlmClient>>,
        options: CheckOptions,
    ) -> Result<Self> {
        // `resolve_trusted` verifies the trust block of the root and every
        // transitive ancestor reachable through `extends:`. This is the only
        // gate before `script:` rules may run, so the trust chain must be
        // verified end-to-end here. It also detects schema v1 (P2-11) before
        // trust verify and surfaces a `hector migrate` hint.
        let config = crate::config::extends::resolve_trusted(config_path)?;

        // Validate every rule's scope by constructing the matcher up front.
        for (rule_id, rule) in &config.rules {
            crate::config::scope::ScopeMatcher::new(&rule.scope)
                .with_context(|| format!("rule `{rule_id}` has invalid scope glob"))?;
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

        Ok(Self {
            config,
            config_dir,
            llm,
            skip,
            options,
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

    /// Evaluate a single rule against a single file. Pure helper extracted
    /// so the parallel dispatch in `check()` can `par_iter().map(…)` over
    /// it. Owns nothing — every input is borrowed; output is three owned
    /// fields that merge cleanly via `extend`/`push` post-iteration.
    ///
    /// C4: when `inputs.collect_explain` is true, the outcome carries a
    /// `RuleExplain` row describing the disposition of the rule (fire /
    /// pass / dispatched / skipped). Out-of-scope rules return early with
    /// `explain: None` because they don't appear in the explain report.
    fn evaluate_one_rule(
        &self,
        rule_id: &str,
        rule: &Rule,
        inputs: &CheckInputs<'_>,
    ) -> RuleOutcome {
        let matcher =
            crate::config::scope::ScopeMatcher::new(&rule.scope).expect("scope validated at load");
        if !matcher.matches(inputs.match_path) {
            return RuleOutcome {
                violations: vec![],
                passed: None,
                explain: None,
                record: None,
            };
        }
        // A3: short-circuit semantic dispatch when the diff cannot
        // plausibly match — see `try_semantic_skip`. The returned reason
        // string also feeds the explain row so authors see the same
        // string the telemetry recorded.
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
            content: if inputs.content.is_empty() {
                None
            } else {
                Some(inputs.content)
            },
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
        // D1: when a semantic rule reached the LLM and produced a
        // result, emit a SemanticVerdict telemetry line. Errors don't
        // produce a verdict line — those surface as engine_error in the
        // per-rule record.
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

    /// D1: emit a SemanticVerdict telemetry line. Used by the semantic
    /// dispatch arm of `evaluate_one_rule` and by `check_session` when a
    /// session rule reaches the LLM. Best-effort: failures stderr-warn.
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

    /// Post-process the engine's `Result<Vec<Violation>>` into a `RuleOutcome`.
    /// Applies disable-directive suppression and converts engine errors into
    /// `Engine::Internal` violations. Split out of `evaluate_one_rule` to
    /// keep the per-rule cognitive complexity well below the workspace cap.
    ///
    /// C4: when `inputs.collect_explain` is true, the outcome carries a
    /// `RuleExplain` row whose `outcome` is derived from
    /// `(engine_errored, any_emitted, engine_kind)` by
    /// [`explain_outcome_for`].
    fn merge_engine_outcome(
        rule_id: &str,
        engine: EngineKind,
        inputs: &CheckInputs<'_>,
        outcome: Result<Vec<Violation>>,
        elapsed: u64,
    ) -> RuleOutcome {
        let verdict_engine = engine_kind_to_verdict_engine(engine);
        match outcome {
            // P1-11: the engine may return many violations (AST emits one
            // per match). Walk the vec, apply per-violation disable
            // directives, and only record the rule as passed if every match
            // was suppressed (or there were none to begin with).
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
                // P1-1: engine runtime errors are Engine::Internal, not
                // Engine::Trust. Trust failures halt at load time and never
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
    ///
    /// Thin wrapper over `check_inner` that drops the explain rows; the
    /// public signature is held stable so callers don't have to opt into
    /// the C4 explain shape unless they want it.
    pub fn check(&self, input: CheckInput) -> Result<Verdict> {
        self.check_inner(input, false).map(|r| r.verdict)
    }

    /// C4: like [`Self::check`], but returns a per-rule outcome list
    /// when the engine was built with `CheckOptions { explain: true, .. }`.
    /// With explain off, the returned `explain` list is empty.
    pub fn check_with_explain(&self, input: CheckInput) -> Result<CheckReport> {
        self.check_inner(input, self.options.explain)
    }

    // Central orchestration: input-mode normalization, skip short-circuit,
    // four-engine dispatch, telemetry. Decomposing further would split the
    // flow across helpers without making any individual piece easier to
    // reason about; the complexity is intrinsic to the work this method
    // does, not an accident.
    #[allow(clippy::cognitive_complexity)]
    fn check_inner(&self, input: CheckInput, collect_explain: bool) -> Result<CheckReport> {
        use crate::disable::DisableMap;
        let start = Instant::now();
        let (path, content, diff) = match input {
            CheckInput::File { path, content } => (path, content, String::new()),
            CheckInput::Diff { file, unified_diff } => {
                // Read the post-edit file from disk so AST rules, disable
                // directives, and any other content-based engine see real
                // content. In the agent flow, diff mode runs *after* the
                // agent's edit has landed on disk, so reading the file here
                // is the correct semantics (P0-5, P0-7). A missing file
                // falls back to empty content — AST will then no-op rather
                // than crashing the runner.
                let content = std::fs::read_to_string(&file).unwrap_or_default();
                (file, content, unified_diff)
            }
        };

        if self.skip.matches(&path) {
            let elapsed = start.elapsed().as_millis() as u64;
            let verdict = Verdict {
                schema_version: crate::verdict::SCHEMA_VERSION,
                hector_version: env!("CARGO_PKG_VERSION").to_string(),
                status: crate::verdict::Status::Pass,
                violations: vec![],
                passed_checks: vec![],
                elapsed_ms: elapsed,
            };
            // P2-21: surface telemetry append failures (disk-full,
            // unwritable path, FS lock issues) instead of silently
            // swallowing them. The check itself still succeeds; the
            // append is best-effort and never the source of truth.
            if let Err(e) = crate::telemetry::append(
                &self.config_dir.join(".hector/log.jsonl"),
                &LogEntry::Check {
                    ts: chrono::Utc::now().to_rfc3339(),
                    file: path.display().to_string(),
                    status: Status::Pass,
                    elapsed_ms: elapsed,
                    rules: vec![],
                },
            ) {
                eprintln!("hector: telemetry append failed: {e:#}");
            }
            return Ok(CheckReport {
                verdict,
                explain: Vec::new(),
                deferred: None,
            });
        }

        let disable_map = DisableMap::from_source(&content);

        let mut violations: Vec<Violation> = Vec::new();
        let mut passed: Vec<String> = Vec::new();
        let mut explain: Vec<RuleExplain> = Vec::new();

        let match_path = relativize(&path, &self.config_dir);

        let inputs = CheckInputs {
            match_path: &match_path,
            path: &path,
            content: &content,
            diff: &diff,
            disable_map: &disable_map,
            collect_explain,
        };

        // C4: apply the `--rule` filter *upstream* of the parallel
        // dispatch so filtered-out rules never enter the work queue and
        // never trigger their engine (in particular, no LLM call). Empty
        // set = run every rule. The collected pair list also keeps the
        // single-rule fast-path measurable (skip pool construction when
        // the filter narrows down to one rule).
        //
        // H1: when `emit_semantic_payload` is set, partition out the
        // semantic/session rules into `deferred_rules` and drop them
        // from the dispatch queue. Deterministic engines (script, ast)
        // still run on the same per-file path. In-scope deferred rules
        // are recorded for the envelope; out-of-scope deferred rules
        // never enter the deferred payload (same scope discipline as
        // the dispatch path).
        let filter: &HashSet<String> = &self.options.rules;
        let mut selected: Vec<(&String, &Rule)> = Vec::new();
        let mut deferred_rules: Vec<crate::verdict_deferred::DeferredRule> = Vec::new();
        for (rule_id, rule) in &self.config.rules {
            if !filter.is_empty() && !filter.contains(rule_id.as_str()) {
                continue;
            }
            if should_defer(rule.engine, &self.options) {
                let matcher = crate::config::scope::ScopeMatcher::new(&rule.scope)
                    .expect("scope validated at load");
                if !matcher.matches(&match_path) {
                    continue;
                }
                deferred_rules.push(crate::verdict_deferred::DeferredRule {
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
                    explain.push(RuleExplain {
                        rule_id: rule_id.clone(),
                        engine: rule.engine,
                        outcome: ExplainOutcome::Skipped {
                            reason: "deferred_subagent".into(),
                        },
                    });
                }
                continue;
            }
            selected.push((rule_id, rule));
        }

        // B1: dispatch rules in parallel. Output order matches input
        // (`BTreeMap` key order, preserved by the partitioning loop
        // above) — `par_iter().collect::<Vec<_>>()` is deterministic.
        // Single-rule workloads skip pool construction entirely.
        let outcomes: Vec<RuleOutcome> = if selected.len() <= 1 {
            selected
                .iter()
                .map(|(rule_id, rule)| self.evaluate_one_rule(rule_id, rule, &inputs))
                .collect()
        } else {
            let pool = self.execution_pool();
            pool.install(|| {
                selected
                    .par_iter()
                    .map(|(rule_id, rule)| self.evaluate_one_rule(rule_id, rule, &inputs))
                    .collect()
            })
        };

        let mut records: Vec<PerRuleRecord> = Vec::new();
        for outcome in outcomes {
            violations.extend(outcome.violations);
            if let Some(id) = outcome.passed {
                passed.push(id);
            }
            if let Some(row) = outcome.explain {
                explain.push(row);
            }
            if let Some(rec) = outcome.record {
                records.push(rec);
            }
        }

        // P2-6: a corrupt or unreadable baseline used to fall through
        // `unwrap_or_default()` silently — operators got unrelated
        // suppression behavior with no diagnostic. Now: `NotFound` stays
        // silent (the common first-run state), any other load failure
        // surfaces a one-line warning to stderr and we proceed with an
        // empty baseline so the check still runs.
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
        // E1: pass the post-edit file content so the baseline can compare
        // each stored `line_sha256` against the current line text. A
        // missing checksum (legacy v1 entry) falls back to the pre-E1
        // tuple-only match — see `Baseline::contains_with_content`.
        violations.retain(|v| !baseline.contains_with_content(v, Some(&content)));

        let verdict =
            Verdict::from_violations(violations, passed, start.elapsed().as_millis() as u64);
        // P2-21: same rationale as the skip-path append above.
        if let Err(e) = crate::telemetry::append(
            &self.config_dir.join(".hector/log.jsonl"),
            &LogEntry::Check {
                ts: chrono::Utc::now().to_rfc3339(),
                file: path.display().to_string(),
                status: verdict.status,
                elapsed_ms: verdict.elapsed_ms,
                rules: records,
            },
        ) {
            eprintln!("hector: telemetry append failed: {e:#}");
        }
        let deferred =
            self.build_deferred_envelope(deferred_rules, &path, &content, &diff, &verdict);
        Ok(CheckReport {
            verdict,
            explain,
            deferred,
        })
    }

    /// H1: assemble the `DeferredVerdict` envelope from the rules that
    /// were short-circuited by `should_defer`. Returns `None` when the
    /// list is empty so the CLI can branch on a single `Option`. Lives
    /// outside `check_inner` to keep that function's cognitive
    /// complexity below the workspace cap.
    fn build_deferred_envelope(
        &self,
        deferred_rules: Vec<crate::verdict_deferred::DeferredRule>,
        path: &Path,
        content: &str,
        diff: &str,
        verdict: &Verdict,
    ) -> Option<crate::verdict_deferred::DeferredVerdict> {
        if deferred_rules.is_empty() {
            return None;
        }
        // Resolve each deferred id back to its full `Rule` so the
        // evaluator-input string sees the same `(id, &Rule)` slice the
        // direct-API path would have seen.
        let rule_refs: Vec<(&str, &crate::config::Rule)> = deferred_rules
            .iter()
            .filter_map(|d| self.config_rule(&d.id).map(|r| (d.id.as_str(), r)))
            .collect();
        let primary = if diff.is_empty() {
            content.to_string()
        } else {
            diff.to_string()
        };
        let evaluator_input = crate::llm::prompt::build_evaluator_input(&rule_refs, &primary, None);

        // R5: thread the optional evaluator_model override from the
        // loaded `llm:` block into the payload. Only the subagent
        // provider reads this; other providers never construct a
        // deferred envelope so the value would be meaningless anyway.
        let evaluator_model = self
            .config
            .llm
            .as_ref()
            .and_then(|l| l.evaluator_model.clone());

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
            },
            elapsed_ms: verdict.elapsed_ms,
        })
    }

    /// C4: render the LLM prompts that *would* be sent for every in-scope
    /// semantic rule, without dispatching anything. Used by
    /// `hector check --print-prompt` to debug prompt construction without
    /// burning API calls.
    ///
    /// Honors `CheckOptions.rules` (the `--rule` filter) and the per-rule
    /// scope matcher. Skips rules whose engine is not `semantic`. Returns
    /// an empty vec if no semantic rule is in scope.
    pub fn render_semantic_prompts(&self, input: CheckInput) -> Result<Vec<RenderedPrompt>> {
        let (path, diff) = match input {
            CheckInput::File { path, .. } => (path, String::new()),
            CheckInput::Diff { file, unified_diff } => (file, unified_diff),
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
            let matcher = crate::config::scope::ScopeMatcher::new(&rule.scope)
                .expect("scope validated at load");
            if !matcher.matches(&match_path) {
                continue;
            }
            let scope = rule.context.unwrap_or(crate::config::ContextScope::Diff);
            let (primary, context_text) = crate::engine::context::expand_context(
                scope,
                if diff.is_empty() { None } else { Some(&diff) },
                Some(&path),
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

    /// D1: split the per-rule arms of `check_session` out so the loop
    /// body stays under the cognitive-complexity cap. Pushes per-rule
    /// outcomes into the shared accumulators and emits a
    /// `SemanticVerdict` for each rule that actually reached the LLM.
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
            // P1-1: session-engine runtime errors are Engine::Internal.
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
            // P2-17: per-edit scope filtering. The session engine
            // aggregates `state.edits` into one LLM prompt; without
            // filtering, a rule scoped to `src/auth/**` would fire on
            // sessions whose every edit lives under `src/billing/`. We
            // construct the rule's scope matcher (validated at load) and
            // keep only edits whose file path matches. If nothing
            // matches, the rule trivially passes without an LLM call.
            let matcher = crate::config::scope::ScopeMatcher::new(&rule.scope)
                .expect("scope validated at load");
            let filtered_edits: Vec<crate::session_state::EditRecord> = state
                .edits
                .iter()
                .filter(|e| matcher.matches(std::path::Path::new(&e.file)))
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
        // P2-21: same rationale as the per-file append above.
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
}
