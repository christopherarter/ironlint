//! H2: `hector record-verdict` — append a single `SemanticVerdict`
//! record to `.hector/log.jsonl`. Consumed by the Claude Code
//! interpreter skill after a subagent evaluates a deferred semantic
//! rule.

use anyhow::Result;
use clap::ValueEnum;
use std::path::Path;

/// Two-arm enum enforcing `--verdict pass | violation` at clap-parse
/// time. Anything else is a parse error from clap — the runtime body
/// of [`run`] cannot see an invalid value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum VerdictValue {
    Pass,
    Violation,
}

impl VerdictValue {
    fn as_wire_str(self) -> &'static str {
        // The on-disk wire format mirrors bully's `pass` / `violation`
        // (lowercase). The `LogEntry::SemanticVerdict.verdict: String`
        // field is intentionally stringly-typed at the telemetry layer
        // so future extensions don't require a schema bump.
        match self {
            Self::Pass => "pass",
            Self::Violation => "violation",
        }
    }
}

pub fn run(rule: String, verdict: VerdictValue, file: Option<String>, dir: &Path) -> Result<i32> {
    let log_path = dir.join(".hector/log.jsonl");

    // Lazy session_init: if the log is empty/absent, write a session_init
    // record first so the log starts with the canonical first-record type.
    // Idempotent in the same sense as `hector session record` — we check
    // the file presence; we do not parse existing records to avoid an O(n)
    // read for every append.
    if !log_path.exists()
        || std::fs::metadata(&log_path)
            .map(|m| m.len() == 0)
            .unwrap_or(true)
    {
        let init = hector_core::telemetry::LogEntry::SessionInit {
            ts: chrono::Utc::now().to_rfc3339(),
            hector_version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: hector_core::telemetry::SCHEMA_VERSION,
        };
        if let Err(e) = hector_core::telemetry::append(&log_path, &init) {
            eprintln!("ERROR: failed to write session_init: {e:#}");
            return Ok(1);
        }
    }

    let entry = hector_core::telemetry::LogEntry::SemanticVerdict {
        ts: chrono::Utc::now().to_rfc3339(),
        rule,
        verdict: verdict.as_wire_str().to_string(),
        file,
    };

    if let Err(e) = hector_core::telemetry::append(&log_path, &entry) {
        eprintln!("ERROR: failed to append semantic_verdict: {e:#}");
        return Ok(1);
    }

    Ok(0)
}
