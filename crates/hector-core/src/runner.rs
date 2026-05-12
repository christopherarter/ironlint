use crate::config::skip::{parse_user_global_ignore, SkipMatcher, USER_GLOBAL_IGNORE_FILENAME};
use crate::config::{Config, EngineKind, Rule};
use crate::engine::{RuleContext, RuleEngine};
use crate::verdict::{Verdict, Violation};
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Per-rule evaluation result, before the runner-level baseline pass.
///
/// `passed` is `Some(rule_id)` when the rule produced no emitted violations
/// (no match, no engine output, or every match suppressed by a disable
/// directive); `None` otherwise. Splitting passed/violations from one
/// `Result<Vec<Violation>>` keeps the parallel `collect` straightforward.
struct RuleOutcome {
    violations: Vec<Violation>,
    passed: Option<String>,
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
}

pub struct HectorEngine {
    config: Config,
    config_dir: PathBuf,
    llm: Option<Box<dyn crate::llm::LlmClient>>,
    skip: SkipMatcher,
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

pub struct HectorEngineBuilder {
    llm: Option<Box<dyn crate::llm::LlmClient>>,
}

impl HectorEngineBuilder {
    pub fn new() -> Self {
        Self { llm: None }
    }

    pub fn with_llm(mut self, llm: Box<dyn crate::llm::LlmClient>) -> Self {
        self.llm = Some(llm);
        self
    }

    pub fn load(self, config_path: &Path) -> Result<HectorEngine> {
        HectorEngine::load_with(config_path, self.llm)
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

impl HectorEngine {
    pub fn load(config_path: &Path) -> Result<Self> {
        Self::load_with(config_path, None)
    }

    pub fn builder() -> HectorEngineBuilder {
        HectorEngineBuilder::new()
    }

    fn load_with(
        config_path: &Path,
        llm_override: Option<Box<dyn crate::llm::LlmClient>>,
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
    /// it. Owns nothing — every input is borrowed; output is two owned
    /// collections that merge cleanly via `extend`/`push` post-iteration.
    fn evaluate_one_rule(
        &self,
        rule_id: &str,
        rule: &Rule,
        inputs: &CheckInputs<'_>,
    ) -> RuleOutcome {
        let matcher = crate::config::scope::ScopeMatcher::new(&rule.scope)
            .expect("scope validated at load");
        if !matcher.matches(inputs.match_path) {
            return RuleOutcome {
                violations: vec![],
                passed: None,
            };
        }
        // A3: short-circuit semantic dispatch when the diff cannot
        // plausibly match — see `try_semantic_skip`.
        if self.try_semantic_skip(rule_id, rule, inputs.path, inputs.diff) {
            return RuleOutcome {
                violations: vec![],
                passed: Some(rule_id.to_string()),
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
        let outcome: Result<Vec<Violation>> = match rule.engine {
            EngineKind::Script => crate::engine::script::ScriptEngine.run(&ctx),
            EngineKind::Ast => crate::engine::ast::AstEngine.run(&ctx),
            EngineKind::Semantic => crate::engine::semantic::SemanticEngine.run(&ctx),
            // Session is dispatched via `check_session`, not the per-file
            // path; treat it as a pass here.
            _ => Ok(Vec::new()),
        };
        Self::merge_engine_outcome(rule_id, inputs, outcome)
    }

    /// Post-process the engine's `Result<Vec<Violation>>` into a `RuleOutcome`.
    /// Applies disable-directive suppression and converts engine errors into
    /// `Engine::Internal` violations. Split out of `evaluate_one_rule` to
    /// keep the per-rule cognitive complexity well below the workspace cap.
    fn merge_engine_outcome(
        rule_id: &str,
        inputs: &CheckInputs<'_>,
        outcome: Result<Vec<Violation>>,
    ) -> RuleOutcome {
        match outcome {
            // P1-11: the engine may return many violations (AST emits one
            // per match). Walk the vec, apply per-violation disable
            // directives, and only record the rule as passed if every match
            // was suppressed (or there were none to begin with).
            Ok(vs) if vs.is_empty() => RuleOutcome {
                violations: vec![],
                passed: Some(rule_id.to_string()),
            },
            Ok(vs) => Self::apply_disables(rule_id, inputs.disable_map, vs),
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
                RuleOutcome {
                    violations: vec![v],
                    passed: None,
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
        disable_map: &crate::disable::DisableMap,
        vs: Vec<Violation>,
    ) -> RuleOutcome {
        let mut kept: Vec<Violation> = Vec::new();
        let mut any_emitted = false;
        for v in vs {
            let disabled = match v.line {
                Some(line) => disable_map.is_disabled(line, rule_id),
                None => disable_map.is_disabled_file_wide(rule_id),
            };
            if disabled {
                continue;
            }
            kept.push(v);
            any_emitted = true;
        }
        let passed = if any_emitted {
            None
        } else {
            // Every match was suppressed by a disable directive — treat the
            // rule as passing for this file so it shows up in
            // `passed_checks` and telemetry.
            Some(rule_id.to_string())
        };
        RuleOutcome {
            violations: kept,
            passed,
        }
    }

    // Central orchestration: input-mode normalization, skip short-circuit,
    // four-engine dispatch, telemetry. Decomposing further would split the
    // flow across helpers without making any individual piece easier to
    // reason about; the complexity is intrinsic to the work this method
    // does, not an accident.
    #[allow(clippy::cognitive_complexity)]
    pub fn check(&self, input: CheckInput) -> Result<Verdict> {
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
                &crate::telemetry::LogEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    kind: "skipped".into(),
                    file: path.display().to_string(),
                    rule_id: None,
                    status: "pass".into(),
                    elapsed_ms: elapsed,
                    reason: None,
                },
            ) {
                eprintln!("hector: telemetry append failed: {e:#}");
            }
            return Ok(verdict);
        }

        let disable_map = DisableMap::from_source(&content);

        let mut violations: Vec<Violation> = Vec::new();
        let mut passed: Vec<String> = Vec::new();

        let match_path = relativize(&path, &self.config_dir);

        let inputs = CheckInputs {
            match_path: &match_path,
            path: &path,
            content: &content,
            diff: &diff,
            disable_map: &disable_map,
        };

        // B1: dispatch rules in parallel. Output order matches input
        // (`BTreeMap` key order) — `par_iter().collect::<Vec<_>>()` is
        // deterministic. Single-rule configs skip pool construction entirely.
        let outcomes: Vec<RuleOutcome> = if self.config.rules.len() <= 1 {
            self.config
                .rules
                .iter()
                .map(|(rule_id, rule)| self.evaluate_one_rule(rule_id, rule, &inputs))
                .collect()
        } else {
            let pool = self.execution_pool();
            let rules: Vec<(&String, &Rule)> = self.config.rules.iter().collect();
            pool.install(|| {
                rules
                    .par_iter()
                    .map(|(rule_id, rule)| self.evaluate_one_rule(rule_id, rule, &inputs))
                    .collect()
            })
        };

        for outcome in outcomes {
            violations.extend(outcome.violations);
            if let Some(id) = outcome.passed {
                passed.push(id);
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
            &crate::telemetry::LogEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                kind: "check".into(),
                file: path.display().to_string(),
                rule_id: None,
                status: format!("{:?}", verdict.status).to_lowercase(),
                elapsed_ms: verdict.elapsed_ms,
                reason: None,
            },
        ) {
            eprintln!("hector: telemetry append failed: {e:#}");
        }
        Ok(verdict)
    }

    /// If the rule is semantic and the diff cannot plausibly match it,
    /// record a `semantic_skipped` telemetry entry and return `true` so
    /// the caller skips engine dispatch. Otherwise return `false`.
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
    fn try_semantic_skip(&self, rule_id: &str, rule: &Rule, path: &Path, diff: &str) -> bool {
        if rule.engine != EngineKind::Semantic || diff.is_empty() {
            return false;
        }
        let analysis = crate::diff::analysis::can_match_diff(diff, path, &rule.description);
        let crate::diff::analysis::CanMatch::No(reason) = analysis else {
            return false;
        };
        let log_path = self.config_dir.join(".hector/log.jsonl");
        let entry = crate::telemetry::LogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            kind: "semantic_skipped".into(),
            file: path.display().to_string(),
            rule_id: Some(rule_id.to_string()),
            status: "pass".into(),
            elapsed_ms: 0,
            reason: Some(reason.as_str().to_string()),
        };
        if let Err(e) = crate::telemetry::append(&log_path, &entry) {
            eprintln!("hector: telemetry append failed: {e:#}");
        }
        true
    }

    pub fn check_session(
        &self,
        state: &crate::session_state::SessionState,
    ) -> Result<crate::verdict::Verdict> {
        use crate::engine::session::SessionEngine;

        let start = Instant::now();
        let mut violations = Vec::new();
        let mut passed = Vec::new();
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
            match session_engine.evaluate(&scoped_state, rule_id, rule, llm) {
                Ok(Some(v)) => violations.push(v),
                Ok(None) => passed.push(rule_id.clone()),
                // P1-1: session-engine runtime errors are Engine::Internal.
                Err(e) => violations.push(crate::verdict::Violation {
                    rule_id: format!("{rule_id}__internal"),
                    severity: crate::verdict::Severity::Error,
                    engine: crate::verdict::Engine::Internal,
                    file: "".to_string(),
                    line: None,
                    column: None,
                    message: format!("{e:#}"),
                    suggestion: None,
                    context: None,
                }),
            }
        }
        let verdict = crate::verdict::Verdict::from_violations(
            violations,
            passed,
            start.elapsed().as_millis() as u64,
        );
        // P2-21: same rationale as the per-file append above.
        if let Err(e) = crate::telemetry::append(
            &self.config_dir.join(".hector/log.jsonl"),
            &crate::telemetry::LogEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                kind: "check_session".into(),
                file: "".into(),
                rule_id: None,
                status: format!("{:?}", verdict.status).to_lowercase(),
                elapsed_ms: verdict.elapsed_ms,
                reason: None,
            },
        ) {
            eprintln!("hector: telemetry append failed: {e:#}");
        }
        Ok(verdict)
    }
}
