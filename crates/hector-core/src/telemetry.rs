//! Append-only check log at `.hector/log.jsonl`.
//!
//! Typed records: every line is one `LogEntry`. The discriminator is `type`
//! (snake_case). Payload fields are variant-specific. The legacy flat-record
//! reader was removed at the 0.3 redesign (the deprecation window is over).
//!
//! Wire format documented in [`docs/operating/telemetry.md`](../../docs/operating/telemetry.md).
use crate::verdict::Status;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

/// Telemetry record-set version. Independent of the verdict schema.
///
/// Bumps when this enum's shape changes. Bumped to 4 for the checks pipeline
/// redesign: per-gate records became per-check (`gate`→`check` field, `step`
/// added, `event` added to `LogEntry::Check`).
pub const SCHEMA_VERSION: u32 = 4;

/// Per-check outcome line carried inside a [`LogEntry::Check`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerCheckRecord {
    pub check: String,
    /// Step within a multi-step check. `None` in Phase 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    pub status: Status,
    pub elapsed_ms: u64,
    /// Optional reason: a stable `InternalReason` string for crashed checks.
    /// `None` for vanilla pass/block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One line in `.hector/log.jsonl`.
///
/// Discriminator field is `type`; variant payload follows. `Check.checks` is
/// empty when no check matched the file (file was checked, no check ran).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LogEntry {
    Check {
        ts: String,
        file: String,
        event: String,
        status: Status,
        elapsed_ms: u64,
        checks: Vec<PerCheckRecord>,
    },
}

/// Append one record. Atomic single-write; owner-only mode; advisory `flock`
/// to serialize concurrent writers.
pub fn append(path: &Path, entry: &LogEntry) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut opts = OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;

    let mut line = serde_json::to_string(entry)?;
    line.push('\n');

    #[cfg(unix)]
    {
        use fs4::fs_std::FileExt;
        FileExt::lock_exclusive(&file)?;
        let result = file.write_all(line.as_bytes());
        FileExt::unlock(&file)?;
        result?;
    }
    #[cfg(not(unix))]
    file.write_all(line.as_bytes())?;

    Ok(())
}

/// Read every record in `path`. Malformed lines are warned to stderr and
/// dropped — a single corrupt line should not fail the whole batch.
pub fn read_all(path: &Path) -> Result<Vec<LogEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<LogEntry>(line) {
            Ok(entry) => out.push(entry),
            Err(e) => {
                eprintln!(
                    "hector: warning — telemetry log {}:{} dropped (parse error: {e})",
                    path.display(),
                    i + 1
                );
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_check_entry() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        let entry = LogEntry::Check {
            ts: "2026-06-15T00:00:00Z".into(),
            file: "a.rs".into(),
            event: "edit".into(),
            status: Status::Block,
            elapsed_ms: 3,
            checks: vec![PerCheckRecord {
                check: "no-todo".into(),
                step: None,
                status: Status::Block,
                elapsed_ms: 3,
                reason: None,
            }],
        };
        append(&log, &entry).unwrap();
        let back = read_all(&log).unwrap();
        assert_eq!(back, vec![entry]);
    }

    #[test]
    fn schema_version_is_4() {
        assert_eq!(SCHEMA_VERSION, 4);
    }
}
