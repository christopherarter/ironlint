use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub schema_version: u32,
    pub hector_version: String,
    pub status: Status,
    pub violations: Vec<Violation>,
    pub passed_checks: Vec<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Warn,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub rule_id: String,
    pub severity: Severity,
    pub engine: Engine,
    pub file: PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
    pub suggestion: Option<String>,
    pub context: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Script,
    Ast,
    Semantic,
    Session,
    Trust,
}

impl Verdict {
    pub fn pass() -> Self {
        Self {
            schema_version: 1,
            hector_version: env!("CARGO_PKG_VERSION").to_string(),
            status: Status::Pass,
            violations: vec![],
            passed_checks: vec![],
            elapsed_ms: 0,
        }
    }

    pub fn from_violations(violations: Vec<Violation>, passed: Vec<String>, elapsed_ms: u64) -> Self {
        let status = if violations.iter().any(|v| v.severity == Severity::Error) {
            Status::Block
        } else if violations.is_empty() {
            Status::Pass
        } else {
            Status::Warn
        };
        Self {
            schema_version: 1,
            hector_version: env!("CARGO_PKG_VERSION").to_string(),
            status,
            violations,
            passed_checks: passed,
            elapsed_ms,
        }
    }
}
