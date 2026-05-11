use crate::config::{parse_file_with_extends, scope::ScopeMatcher, Config, EngineKind};
use crate::engine::script::run_script_rule;
use crate::trust;
use crate::verdict::{Verdict, Violation};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub struct HectorEngine {
    config: Config,
    config_dir: PathBuf,
}

pub enum CheckInput {
    File { path: PathBuf, content: String },
    Diff { file: PathBuf, unified_diff: String },
}

impl HectorEngine {
    pub fn load(config_path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(config_path)
            .with_context(|| format!("reading {}", config_path.display()))?;
        trust::verify(&raw)?;
        let config = parse_file_with_extends(config_path)?;
        let config_dir = config_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(Self { config, config_dir })
    }

    pub fn check(&self, input: CheckInput) -> Verdict {
        let start = Instant::now();
        let (path, _content, _diff) = match input {
            CheckInput::File { path, content } => (path, content, String::new()),
            CheckInput::Diff { file, unified_diff } => (file, String::new(), unified_diff),
        };

        let mut violations: Vec<Violation> = Vec::new();
        let mut passed: Vec<String> = Vec::new();

        for (rule_id, rule) in &self.config.rules {
            let matcher = match ScopeMatcher::new(&rule.scope) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !matcher.matches(&path) {
                continue;
            }
            let outcome = match rule.engine {
                EngineKind::Script => run_script_rule(rule_id, rule, &path, "", &self.config_dir),
                _ => {
                    // 0.1a foundation: non-script engines are no-ops; plans B/C wire them.
                    Ok(None)
                }
            };
            match outcome {
                Ok(Some(v)) => violations.push(v),
                Ok(None) => passed.push(rule_id.clone()),
                Err(_) => {} // engine error treated as no violation; logged at a higher layer
            }
        }

        Verdict::from_violations(violations, passed, start.elapsed().as_millis() as u64)
    }
}
