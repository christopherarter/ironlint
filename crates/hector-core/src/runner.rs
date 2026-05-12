use crate::config::skip::{parse_user_global_ignore, SkipMatcher, USER_GLOBAL_IGNORE_FILENAME};
use crate::config::{parse_file_with_extends, Config, EngineKind};
use crate::engine::script::run_script_rule;
use crate::trust;
use crate::verdict::{Verdict, Violation};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;

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
/// authored in `config_dir`-relative terms. Falls back to the original path
/// when canonicalization is impossible (e.g. the file is absent during diff
/// mode where the diff references a path not yet on disk).
fn relativize(path: &std::path::Path, root: &std::path::Path) -> std::path::PathBuf {
    use std::path::PathBuf;
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
        let raw = std::fs::read_to_string(config_path)
            .with_context(|| format!("reading {}", config_path.display()))?;
        trust::verify(&raw)?;
        let config = parse_file_with_extends(config_path)?;

        // Validate every rule's scope by constructing the matcher up front.
        for (rule_id, rule) in &config.rules {
            crate::config::scope::ScopeMatcher::new(&rule.scope)
                .with_context(|| format!("rule `{rule_id}` has invalid scope glob"))?;
        }

        if crate::config::parser::is_legacy(&config) {
            eprintln!(
                "hector: warning — `.bully.yml` schema_version 1 is deprecated; run `hector migrate` to upgrade to schema_version 2"
            );
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

    pub fn check(&self, input: CheckInput) -> Result<Verdict> {
        use crate::disable::DisableMap;
        let start = Instant::now();
        let (path, content, diff) = match input {
            CheckInput::File { path, content } => (path, content, String::new()),
            CheckInput::Diff { file, unified_diff } => (file, String::new(), unified_diff),
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
            let _ = crate::telemetry::append(
                &self.config_dir.join(".hector/log.jsonl"),
                &crate::telemetry::LogEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    kind: "skipped".into(),
                    file: path.display().to_string(),
                    rule_id: None,
                    status: "pass".into(),
                    elapsed_ms: elapsed,
                },
            );
            return Ok(verdict);
        }

        let disable_map = DisableMap::from_source(&content);

        let mut violations: Vec<Violation> = Vec::new();
        let mut passed: Vec<String> = Vec::new();

        let match_path = relativize(&path, &self.config_dir);

        for (rule_id, rule) in &self.config.rules {
            let matcher = crate::config::scope::ScopeMatcher::new(&rule.scope)
                .expect("scope validated at load");
            if !matcher.matches(&match_path) {
                continue;
            }
            let outcome = match rule.engine {
                EngineKind::Script => {
                    run_script_rule(rule_id, rule, &path, &diff, &self.config_dir)
                }
                EngineKind::Ast => {
                    use crate::engine::ast::AstEngine;
                    use crate::engine::{RuleContext, RuleEngine};
                    let engine = AstEngine;
                    let ctx = RuleContext {
                        rule_id,
                        rule,
                        file: &path,
                        content: if content.is_empty() {
                            None
                        } else {
                            Some(&content)
                        },
                        diff: if diff.is_empty() { None } else { Some(&diff) },
                        cwd: &self.config_dir,
                        llm: self.llm.as_deref(),
                    };
                    engine.run(&ctx)
                }
                EngineKind::Semantic => {
                    use crate::engine::semantic::SemanticEngine;
                    use crate::engine::{RuleContext, RuleEngine};
                    let engine = SemanticEngine;
                    let ctx = RuleContext {
                        rule_id,
                        rule,
                        file: &path,
                        content: if content.is_empty() {
                            None
                        } else {
                            Some(&content)
                        },
                        diff: if diff.is_empty() { None } else { Some(&diff) },
                        cwd: &self.config_dir,
                        llm: self.llm.as_deref(),
                    };
                    engine.run(&ctx)
                }
                _ => Ok(None),
            };
            match outcome {
                Ok(Some(v)) => {
                    if let Some(line) = v.line {
                        if disable_map.is_disabled(line, rule_id) {
                            passed.push(rule_id.clone());
                            continue;
                        }
                    }
                    violations.push(v);
                }
                Ok(None) => passed.push(rule_id.clone()),
                Err(e) => {
                    violations.push(Violation {
                        rule_id: format!("{rule_id}__internal"),
                        severity: crate::verdict::Severity::Error,
                        engine: crate::verdict::Engine::Trust,
                        file: path.display().to_string(),
                        line: None,
                        column: None,
                        message: format!("{e:#}"),
                        suggestion: None,
                        context: None,
                    });
                }
            }
        }

        let baseline =
            crate::baseline::Baseline::load(&self.config_dir.join(".hector/baseline.json"))
                .unwrap_or_default();
        violations.retain(|v| !baseline.contains(v));

        let verdict =
            Verdict::from_violations(violations, passed, start.elapsed().as_millis() as u64);
        let _ = crate::telemetry::append(
            &self.config_dir.join(".hector/log.jsonl"),
            &crate::telemetry::LogEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                kind: "check".into(),
                file: path.display().to_string(),
                rule_id: None,
                status: format!("{:?}", verdict.status).to_lowercase(),
                elapsed_ms: verdict.elapsed_ms,
            },
        );
        Ok(verdict)
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
            let llm = self.llm.as_deref().ok_or_else(|| {
                anyhow::anyhow!("session check requires LlmClient; build engine with .with_llm()")
            })?;
            match session_engine.evaluate(state, rule_id, rule, llm) {
                Ok(Some(v)) => violations.push(v),
                Ok(None) => passed.push(rule_id.clone()),
                Err(e) => violations.push(crate::verdict::Violation {
                    rule_id: format!("{rule_id}__internal"),
                    severity: crate::verdict::Severity::Error,
                    engine: crate::verdict::Engine::Trust,
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
        let _ = crate::telemetry::append(
            &self.config_dir.join(".hector/log.jsonl"),
            &crate::telemetry::LogEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                kind: "check_session".into(),
                file: "".into(),
                rule_id: None,
                status: format!("{:?}", verdict.status).to_lowercase(),
                elapsed_ms: verdict.elapsed_ms,
            },
        );
        Ok(verdict)
    }
}
