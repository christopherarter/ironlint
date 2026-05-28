//! `hector record-verdict` — append a single `SemanticVerdict` record
//! to `.hector/log.jsonl`. Consumed by the Claude Code interpreter
//! skill after a subagent evaluates a deferred semantic rule.

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn as_wire_str_pass_and_violation() {
        assert_eq!(VerdictValue::Pass.as_wire_str(), "pass");
        assert_eq!(VerdictValue::Violation.as_wire_str(), "violation");
    }

    #[test]
    fn run_appends_to_fresh_dir() {
        let tmp = tempdir().unwrap();
        let code = run("r1".to_string(), VerdictValue::Pass, None, tmp.path()).unwrap();
        assert_eq!(code, 0);
        let content = fs::read_to_string(tmp.path().join(".hector/log.jsonl")).unwrap();
        assert!(content.contains("session_init"));
        assert!(content.contains("semantic_verdict"));
    }

    #[test]
    fn run_skips_session_init_when_log_has_content() {
        let tmp = tempdir().unwrap();
        // First call: creates session_init + semantic_verdict.
        run("r1".to_string(), VerdictValue::Pass, None, tmp.path()).unwrap();
        let before = fs::read_to_string(tmp.path().join(".hector/log.jsonl")).unwrap();
        let init_count_before = before.matches("session_init").count();

        // Second call: only semantic_verdict appended; session_init count unchanged.
        run(
            "r2".to_string(),
            VerdictValue::Violation,
            Some("src/lib.rs".to_string()),
            tmp.path(),
        )
        .unwrap();
        let after = fs::read_to_string(tmp.path().join(".hector/log.jsonl")).unwrap();
        assert_eq!(
            after.matches("session_init").count(),
            init_count_before,
            "session_init must not be written twice",
        );
        assert!(after.contains("\"verdict\":\"violation\""));
    }

    #[test]
    fn run_writes_session_init_for_empty_existing_file() {
        // Cover the `m.len() == 0` branch: file exists but is empty.
        let tmp = tempdir().unwrap();
        let log_dir = tmp.path().join(".hector");
        fs::create_dir_all(&log_dir).unwrap();
        let log_path = log_dir.join("log.jsonl");
        fs::write(&log_path, b"").unwrap(); // empty file
        assert!(log_path.exists());

        let code = run("r1".to_string(), VerdictValue::Pass, None, tmp.path()).unwrap();
        assert_eq!(code, 0);
        let content = fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("session_init"));
    }

    #[cfg(unix)]
    #[test]
    fn run_returns_1_when_second_append_fails() {
        // First write succeeds; then we make the log read-only so the second
        // append (the semantic_verdict) fails — exercises the `Err` arm on line 64.
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir().unwrap();
        let log_dir = tmp.path().join(".hector");
        fs::create_dir_all(&log_dir).unwrap();
        let log_path = log_dir.join("log.jsonl");

        // Pre-populate so the session_init branch is skipped.
        let init = hector_core::telemetry::LogEntry::SessionInit {
            ts: "2024-01-01T00:00:00Z".to_string(),
            hector_version: "0.0.0".to_string(),
            schema_version: hector_core::telemetry::SCHEMA_VERSION,
        };
        hector_core::telemetry::append(&log_path, &init).unwrap();

        // Make the log file read-only so the next write fails.
        let mut perms = fs::metadata(&log_path).unwrap().permissions();
        perms.set_mode(0o444);
        fs::set_permissions(&log_path, perms).unwrap();

        let code = run("r1".to_string(), VerdictValue::Pass, None, tmp.path()).unwrap();
        assert_eq!(
            code, 1,
            "run must return 1 when semantic_verdict append fails"
        );

        // Restore write permission so tempdir cleanup succeeds.
        let mut perms = fs::metadata(&log_path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&log_path, perms).unwrap();
    }
}
