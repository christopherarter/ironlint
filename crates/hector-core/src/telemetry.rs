//! Append-only check log at `.hector/log.jsonl`.
//!
//! Typed records: every line is one `LogEntry`. The discriminator is `type`
//! (snake_case to match the rest of the spec); payload fields are
//! variant-specific.
//!
//! **Backwards compat:** the [`read_all`] reader accepts the legacy flat
//! shape (`{ "kind": "...", "timestamp": "...", ... }`) during a
//! deprecation window that ends at the 0.3 verdict freeze, when this
//! fallback is removed. The writer cannot produce the legacy shape — only
//! the new enum is `Serialize`.
//!
//! Wire format documented in [`docs/operating/telemetry.md`](../../docs/operating/telemetry.md).
use crate::verdict::{Engine, Status};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

/// Telemetry record-set version. Independent of the verdict schema; bumps
/// when this enum's shape changes (added/removed variants or fields).
pub const SCHEMA_VERSION: u32 = 2;

/// Per-rule outcome line carried inside a [`LogEntry::Check`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerRuleRecord {
    pub rule_id: String,
    pub engine: Engine,
    pub status: Status,
    pub elapsed_ms: u64,
    /// Optional reason: `"engine_error"` for runtime failures,
    /// `"disabled"` for `hector-disable:`-suppressed rows. `None` for
    /// vanilla pass/fire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One line in `.hector/log.jsonl`.
///
/// Discriminator field is `type`; variant payload follows. `Check.rules`
/// is an empty vec when the file was short-circuited by a skip pattern
/// (file was checked, no rule ran).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LogEntry {
    Check {
        ts: String,
        file: String,
        status: Status,
        elapsed_ms: u64,
        rules: Vec<PerRuleRecord>,
    },
}

/// Has the legacy-format deprecation warning been emitted in this process?
static LEGACY_WARNING_EMITTED: OnceLock<()> = OnceLock::new();

/// Legacy flat shape. Read-only; never serialized. Only the fields needed to
/// reconstruct a [`LogEntry::Check`] are kept — serde ignores the rest
/// (`kind`, `rule_id`, `reason`) on legacy lines.
#[derive(Deserialize)]
struct LogEntryLegacy {
    timestamp: String,
    file: String,
    status: String,
    elapsed_ms: u64,
}

/// Wrapper deserializer: try the typed shape first, fall back to legacy.
/// Untagged means serde will pick whichever variant fully matches.
#[derive(Deserialize)]
#[serde(untagged)]
enum LogEntryRead {
    Typed(LogEntry),
    Legacy(LogEntryLegacy),
}

impl LogEntryLegacy {
    /// Lift a flat legacy record into a typed [`LogEntry::Check`]. The mapping
    /// is lossy: the legacy format never carried `rules`, and the removed
    /// `semantic_*`/`session_init` kinds have no typed equivalent since LLM
    /// evaluation was dropped — every legacy `kind` collapses to `Check`.
    /// Status string is best-effort; missing/unknown → Pass.
    fn into_typed(self) -> LogEntry {
        LogEntry::Check {
            ts: self.timestamp,
            file: self.file,
            status: parse_status(&self.status),
            elapsed_ms: self.elapsed_ms,
            rules: Vec::new(),
        }
    }
}

fn parse_status(s: &str) -> Status {
    match s {
        "warn" => Status::Warn,
        "block" => Status::Block,
        _ => Status::Pass,
    }
}

fn emit_legacy_warning(path: &Path) {
    if LEGACY_WARNING_EMITTED.set(()).is_ok() {
        eprintln!(
            "hector: warning — telemetry log at {} contains pre-D1 (flat) records; \
             reading them through the legacy fallback. The fallback will be removed \
             at the 0.3 freeze.",
            path.display()
        );
    }
}

/// Append one record. Atomic single-write; owner-only mode; advisory
/// `flock` to serialize concurrent writers (the kernel only guarantees
/// O_APPEND atomicity below `PIPE_BUF`, so we lock for safety).
pub fn append(path: &Path, entry: &LogEntry) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut opts = OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        // Telemetry entries echo back file paths from the user's project, so
        // create owner-only by default rather than inheriting umask (typically
        // 0644).
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;

    // Build the line as a single buffer so the actual write is a single
    // write_all syscall. Two separate write_all calls (line then '\n') leave
    // a window where a concurrent writer can interleave bytes between them.
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');

    // For entries larger than PIPE_BUF (4 KiB on Linux, much smaller on macOS)
    // the kernel's atomic-append guarantee for O_APPEND no longer applies, and
    // concurrent writers can interleave even a single write_all. Serialize
    // writers with an advisory exclusive flock. The cost vs corruption risk
    // is negligible; we hold the lock only for the single write.
    #[cfg(unix)]
    {
        use fs4::fs_std::FileExt;
        FileExt::lock_exclusive(&file)?;
        let result = file.write_all(line.as_bytes());
        // Release explicitly to keep the critical section tight; the lock
        // would also be released when `file` is dropped.
        FileExt::unlock(&file)?;
        result?;
    }
    #[cfg(not(unix))]
    file.write_all(line.as_bytes())?;

    Ok(())
}

/// Read every record in `path`, accepting both v2 (typed) and v1 (legacy
/// flat) shapes. Malformed lines are warned to stderr and dropped — a
/// single corrupt line should not fail the whole batch.
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
        match serde_json::from_str::<LogEntryRead>(line) {
            Ok(LogEntryRead::Typed(entry)) => out.push(entry),
            Ok(LogEntryRead::Legacy(legacy)) => {
                emit_legacy_warning(path);
                out.push(legacy.into_typed());
            }
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
