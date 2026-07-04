//! Append-only check log at `.ironlint/log.jsonl`.
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
/// Bumps when this enum's shape changes. Bumped to 5: `LogEntry::Check.file`
/// became `Option<String>` (absent on pre-commit/set invocations) and
/// `set_size: Option<usize>` was added (present on pre-commit to record the
/// number of files in the checked set).
pub const SCHEMA_VERSION: u32 = 5;

/// Rotate the log once it grows past this many bytes (10 MiB). Keeps exactly
/// one old copy (`log.jsonl.1`); the next append recreates a fresh `log.jsonl`.
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

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

/// One line in `.ironlint/log.jsonl`.
///
/// Discriminator field is `type`; variant payload follows. `Check.checks` is
/// empty when no check matched the file (file was checked, no check ran).
///
/// `file` is present on write-lifecycle invocations (the absolute path of the
/// file being checked) and absent on pre-commit/set invocations where there is
/// no single primary target. `set_size` is the inverse: present on pre-commit
/// with the count of files in the set, absent on per-file write records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LogEntry {
    Check {
        ts: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        set_size: Option<usize>,
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

    // Write-first, rotate-after (design §4): the entry is durably on disk before
    // any rename, so a rotation failure can never lose the just-appended line.
    rotate_if_oversized_with_max(path, MAX_LOG_BYTES);

    Ok(())
}

/// Rotate `path` → `path.1` when it exceeds `max` bytes, keeping one old copy.
///
/// Best-effort and called AFTER the entry is durably written (write-first
/// ordering, design §4): any failure here leaves the just-appended entry intact
/// and rotation simply retries on the next append. Errors are swallowed — a
/// telemetry rotation must never break a check run. `rename` overwrites an
/// existing `.1`, so at most one archived generation is kept.
///
/// The cap is a parameter so tests can drive rotation with a tiny threshold
/// deterministically; the public [`append`] path always passes [`MAX_LOG_BYTES`].
fn rotate_if_oversized_with_max(path: &Path, max: u64) {
    let Ok(meta) = std::fs::metadata(path) else {
        // No file / unstattable → nothing to rotate.
        return;
    };
    if meta.len() <= max {
        return;
    }
    // Append `.1` to the FULL path (filename-agnostic): `…/log.jsonl` → `…/log.jsonl.1`.
    let mut rotated = path.as_os_str().to_os_string();
    rotated.push(".1");
    let _ = std::fs::rename(path, rotated);
}

/// Parse raw JSONL text into entries and dropped lines.
///
/// Returns `(valid_entries, dropped)` where each dropped item is
/// `(line_number_1based, error_string)`. Callers decide what to do with
/// the dropped information (log to stderr, silently discard, etc.).
fn parse_entries(raw: &str) -> (Vec<LogEntry>, Vec<(usize, String)>) {
    let mut entries = Vec::new();
    let mut dropped = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<LogEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(e) => dropped.push((i + 1, e.to_string())),
        }
    }
    (entries, dropped)
}

/// Read every record in `path`. Malformed lines are warned to stderr and
/// dropped — a single corrupt line should not fail the whole batch.
pub fn read_all(path: &Path) -> Result<Vec<LogEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)?;
    let (entries, dropped) = parse_entries(&raw);
    for (line_num, err) in &dropped {
        eprintln!(
            "ironlint: warning — telemetry log {}:{} dropped (parse error: {err})",
            path.display(),
            line_num
        );
    }
    Ok(entries)
}

/// Like [`read_all`] but never writes to stderr on malformed lines.
///
/// Used by the `ironlint watch` event loop, which ticks every ~250 ms while
/// an alternate-screen TUI is active. Any `eprintln!` during that window
/// bleeds through raw mode and corrupts the rendered frame. Dropped lines
/// are silently ignored; only the valid entries are returned.
///
/// Missing file → `Ok(Vec::new())`, same as [`read_all`].
pub fn read_all_quiet(path: &Path) -> Result<Vec<LogEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)?;
    let (entries, _dropped) = parse_entries(&raw);
    Ok(entries)
}

/// Read entries appended to `path` since byte `*offset`, advancing `*offset` to
/// the end of the last COMPLETE line consumed. Returns `(entries, reset)`:
/// - `reset == false`: incremental — `entries` are the newly-appended lines; the
///   caller should EXTEND its accumulated view.
/// - `reset == true`: the file is shorter than `*offset` (rotation/truncation)
///   or missing — `*offset` was reset to 0 and `entries` is the full re-read of
///   the current file (empty if missing); the caller should REPLACE its view.
///
/// Bytes after the last `\n` (a partial line still being written) are NOT
/// consumed — `*offset` is left before them so they are re-read once completed.
/// Malformed lines are dropped silently (same policy as [`read_all_quiet`], so a
/// TUI tick never bleeds `eprintln!` through raw mode).
///
/// `*offset` is mutated only on the success path; on an I/O `Err` it is left
/// unchanged and the error propagates.
pub fn read_since(path: &Path, offset: &mut u64) -> Result<(Vec<LogEntry>, bool)> {
    if !path.exists() {
        *offset = 0;
        return Ok((Vec::new(), true));
    }
    let len = std::fs::metadata(path)?.len();
    if len < *offset {
        // Shrink (rotation from log rotation, or truncation): reset and re-read
        // the whole current file from the top.
        let (entries, consumed) = read_complete_lines_from(path, 0, len)?;
        *offset = consumed;
        return Ok((entries, true));
    }
    // Incremental: consume only the bytes appended since `*offset`.
    let (entries, consumed) = read_complete_lines_from(path, *offset, len)?;
    *offset += consumed;
    Ok((entries, false))
}

/// Seek to `start`, read `[start, len)`, and parse only the lines terminated by
/// a `\n` within that window. Returns `(entries, consumed)` where `consumed` is
/// the byte count up to and INCLUDING the last `\n` (0 when the window holds no
/// complete line — a partial line still mid-write). Bytes past the last `\n` are
/// left unconsumed so they are re-read once completed.
fn read_complete_lines_from(path: &Path, start: u64, len: u64) -> Result<(Vec<LogEntry>, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    // Seek to `start` unconditionally: for `start == 0` this is a no-op (a
    // freshly opened file is already positioned at 0), so a `start > 0` guard
    // would only add a branch with no observable effect.
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity(len.saturating_sub(start) as usize);
    file.read_to_end(&mut buf)?;
    let parsed = match buf.iter().rposition(|&b| b == b'\n') {
        Some(idx) => {
            // Parse through the last `\n`; anything after it is a partial line.
            let text = String::from_utf8_lossy(&buf[..=idx]);
            let (entries, _dropped) = parse_entries(&text);
            (entries, (idx + 1) as u64)
        }
        None => (Vec::new(), 0),
    };
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rotation threshold is a deliberate operational contract (the design
    /// promises "10 MiB"). Rotation *behavior* is exercised via an injectable
    /// cap (`rotate_if_oversized_with_max`), so nothing else pins the production
    /// constant's actual value — a wrong literal (KiB/GiB fat-finger, or a
    /// `*`→`+` slip) would otherwise ship silently.
    #[test]
    fn max_log_bytes_is_ten_mib() {
        assert_eq!(MAX_LOG_BYTES, 10 * 1024 * 1024);
        assert_eq!(MAX_LOG_BYTES, 10_485_760);
    }

    /// Write-lifecycle record: `file` is present, `set_size` is absent.
    #[test]
    fn round_trips_a_write_entry() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        let entry = LogEntry::Check {
            ts: "2026-06-15T00:00:00Z".into(),
            file: Some("a.rs".into()),
            set_size: None,
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
        // Confirm `file` key is present and `set_size` key is absent in the JSON.
        let raw = std::fs::read_to_string(&log).unwrap();
        assert!(raw.contains("\"file\":"), "write record must include file");
        assert!(
            !raw.contains("\"set_size\":"),
            "write record must not include set_size"
        );
    }

    /// Pre-commit/set-level record: `file` is absent, `set_size` is present.
    #[test]
    fn round_trips_a_pre_commit_entry() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        let entry = LogEntry::Check {
            ts: "2026-06-28T00:00:00Z".into(),
            file: None,
            set_size: Some(3),
            event: "pre-commit".into(),
            status: Status::Pass,
            elapsed_ms: 5,
            checks: vec![],
        };
        append(&log, &entry).unwrap();
        let back = read_all(&log).unwrap();
        assert_eq!(back, vec![entry]);
        // Confirm `file` key is absent and `set_size` key is present in the JSON.
        let raw = std::fs::read_to_string(&log).unwrap();
        assert!(
            !raw.contains("\"file\":"),
            "pre-commit record must not include file"
        );
        assert!(
            raw.contains("\"set_size\":3"),
            "pre-commit record must include set_size"
        );
    }

    #[test]
    fn schema_version_is_5() {
        assert_eq!(SCHEMA_VERSION, 5);
    }

    /// Helper: one valid `LogEntry::Check` as a JSONL line.
    fn valid_jsonl_line() -> String {
        let entry = LogEntry::Check {
            ts: "2026-06-29T00:00:00Z".into(),
            file: Some("foo.rs".into()),
            set_size: None,
            event: "write".into(),
            status: Status::Pass,
            elapsed_ms: 1,
            checks: vec![],
        };
        serde_json::to_string(&entry).unwrap()
    }

    /// `read_all` returns the valid entry when a file also contains a malformed
    /// line. The malformed line is dropped (and warned to stderr, but we don't
    /// capture that here — we only care the valid entry survives).
    #[test]
    fn read_all_survives_one_malformed_line() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        let content = format!("{}\nnot-json\n", valid_jsonl_line());
        std::fs::write(&log, content).unwrap();

        let entries = read_all(&log).unwrap();
        assert_eq!(entries.len(), 1, "read_all must return the one valid entry");
        let LogEntry::Check { file, .. } = &entries[0];
        assert_eq!(file.as_deref(), Some("foo.rs"));
    }

    /// `read_all_quiet` returns the valid entry, drops the malformed line
    /// silently, and does not panic.
    #[test]
    fn read_all_quiet_returns_valid_entry_and_drops_malformed_silently() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        let content = format!("{}\nbad-json-line\n", valid_jsonl_line());
        std::fs::write(&log, content).unwrap();

        let entries = read_all_quiet(&log).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "read_all_quiet must return exactly the one valid entry"
        );
        let LogEntry::Check { file, .. } = &entries[0];
        assert_eq!(file.as_deref(), Some("foo.rs"));
    }

    /// `read_all_quiet` returns `Ok([])` for a missing file (no panic, no Err).
    #[test]
    fn read_all_quiet_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("nonexistent.jsonl");
        let result = read_all_quiet(&log).unwrap();
        assert!(result.is_empty());
    }

    /// One `LogEntry::Check` for size-driving tests.
    fn sample_entry() -> LogEntry {
        LogEntry::Check {
            ts: "2026-07-04T00:00:00Z".into(),
            file: Some("foo.rs".into()),
            set_size: None,
            event: "write".into(),
            status: Status::Pass,
            elapsed_ms: 1,
            checks: vec![],
        }
    }

    /// Crossing an injected cap rotates `log.jsonl` → `log.jsonl.1`; the archived
    /// copy holds the pre-rotation entries and the next append recreates a small
    /// fresh `log.jsonl`. Drives rotation with a tiny cap so it stays fast.
    #[test]
    fn rotates_at_injected_cap_and_keeps_one_archive() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        let rotated = dir.path().join("log.jsonl.1");
        let cap = 200u64;

        // Append until the file comfortably exceeds the tiny cap.
        for _ in 0..10 {
            append(&log, &sample_entry()).unwrap();
        }
        let before = std::fs::metadata(&log).unwrap().len();
        assert!(before > cap, "precondition: log must exceed the cap");
        let n_entries = read_all(&log).unwrap().len();

        // Rotate with the injected cap.
        rotate_if_oversized_with_max(&log, cap);

        assert!(rotated.exists(), "rotation must create the .1 archive");
        assert!(
            !log.exists(),
            "current log is renamed away until the next append"
        );
        assert_eq!(
            read_all(&rotated).unwrap().len(),
            n_entries,
            "archive holds every pre-rotation entry"
        );

        // The next append recreates a small, fresh current log.
        append(&log, &sample_entry()).unwrap();
        assert!(log.exists());
        assert_eq!(
            read_all(&log).unwrap().len(),
            1,
            "fresh log holds only the post-rotation entry"
        );
        assert!(
            std::fs::metadata(&log).unwrap().len() < before,
            "fresh log is smaller than the rotated one"
        );
        assert!(
            rotated.exists(),
            "archive is preserved after the fresh append"
        );
    }

    /// Under the cap, no rotation happens and no `.1` archive is created (control).
    #[test]
    fn does_not_rotate_under_cap() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        let rotated = dir.path().join("log.jsonl.1");

        append(&log, &sample_entry()).unwrap();
        // A cap far above one entry: nothing to rotate.
        rotate_if_oversized_with_max(&log, MAX_LOG_BYTES);

        assert!(log.exists(), "log stays in place under the cap");
        assert!(!rotated.exists(), "no archive is created under the cap");
        assert_eq!(read_all(&log).unwrap().len(), 1);
    }

    /// A second rotation overwrites the first `.1` — exactly one archive is kept.
    #[test]
    fn second_rotation_overwrites_prior_archive() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        let rotated = dir.path().join("log.jsonl.1");

        // First generation → rotate → recreate.
        append(&log, &sample_entry()).unwrap();
        rotate_if_oversized_with_max(&log, 0);
        assert!(rotated.exists());
        // Second generation with two entries → rotate again over the tiny cap.
        append(&log, &sample_entry()).unwrap();
        append(&log, &sample_entry()).unwrap();
        rotate_if_oversized_with_max(&log, 0);

        assert_eq!(
            read_all(&rotated).unwrap().len(),
            2,
            "the .1 archive reflects only the most recent rotation"
        );
    }

    /// Rotation is best-effort: a missing file is a no-op (no panic, no `.1`).
    #[test]
    fn rotate_missing_file_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("absent.jsonl");
        rotate_if_oversized_with_max(&log, 0);
        assert!(!dir.path().join("absent.jsonl.1").exists());
    }

    /// Append raw bytes to `path` (used to simulate partial writes and shrink).
    fn append_raw(path: &Path, bytes: &[u8]) {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(bytes).unwrap();
    }

    /// After a first read, `read_since` returns ONLY the newly-appended lines and
    /// advances `offset` to the file end.
    #[test]
    fn read_since_returns_only_new_tail() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        for _ in 0..3 {
            append(&log, &sample_entry()).unwrap();
        }
        let mut offset = 0u64;

        // First read consumes the existing 3 lines (initial full read, not a reset
        // since len >= offset).
        let (first, reset) = read_since(&log, &mut offset).unwrap();
        assert_eq!(first.len(), 3, "first read returns all existing lines");
        assert!(!reset, "len >= offset is incremental, not a reset");
        assert_eq!(
            offset,
            std::fs::metadata(&log).unwrap().len(),
            "offset advances to end of file"
        );

        // Append 2 more; the next read returns EXACTLY those 2, not all 5.
        append(&log, &sample_entry()).unwrap();
        append(&log, &sample_entry()).unwrap();
        let (second, reset2) = read_since(&log, &mut offset).unwrap();
        assert_eq!(second.len(), 2, "second read returns only the new tail");
        assert!(!reset2);
        assert_eq!(offset, std::fs::metadata(&log).unwrap().len());
    }

    /// No new bytes since the last read → empty, no reset, offset unchanged.
    #[test]
    fn read_since_no_new_bytes_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        append(&log, &sample_entry()).unwrap();
        let mut offset = 0u64;
        let _ = read_since(&log, &mut offset).unwrap();
        let at_end = offset;

        let (entries, reset) = read_since(&log, &mut offset).unwrap();
        assert!(entries.is_empty(), "no appends means no new entries");
        assert!(!reset);
        assert_eq!(offset, at_end, "offset does not move without new bytes");
    }

    /// A file shorter than `offset` (rotation/truncation) resets to a full
    /// re-read: `reset == true`, offset back to the new boundary, current content.
    #[test]
    fn read_since_shrink_resets_to_full_reread() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        for _ in 0..4 {
            append(&log, &sample_entry()).unwrap();
        }
        let mut offset = 0u64;
        let _ = read_since(&log, &mut offset).unwrap();
        assert!(offset > 0);

        // Shrink: replace with a single fresh line, far shorter than `offset`.
        let one = format!("{}\n", valid_jsonl_line());
        std::fs::write(&log, &one).unwrap();
        assert!((one.len() as u64) < offset, "precondition: file shrank");

        let (entries, reset) = read_since(&log, &mut offset).unwrap();
        assert!(reset, "a shorter file forces a reset");
        assert_eq!(
            entries.len(),
            1,
            "reset returns the current (small) content"
        );
        assert_eq!(
            offset,
            one.len() as u64,
            "offset resets to the new boundary"
        );
    }

    /// A trailing partial line (no `\n`) is not consumed; once completed it parses
    /// on the next read and the offset advances past it.
    #[test]
    fn read_since_holds_partial_line_until_complete() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        let full = format!("{}\n", valid_jsonl_line());
        let bytes = full.as_bytes();
        let split = bytes.len() / 2;

        // Write the first half only — no newline yet.
        append_raw(&log, &bytes[..split]);
        let mut offset = 0u64;
        let (partial, reset) = read_since(&log, &mut offset).unwrap();
        assert!(partial.is_empty(), "partial line yields no entries");
        assert!(!reset);
        assert_eq!(offset, 0, "offset stays before the incomplete line");

        // Write the rest incl. the newline — now the line is complete.
        append_raw(&log, &bytes[split..]);
        let (done, reset2) = read_since(&log, &mut offset).unwrap();
        assert_eq!(done.len(), 1, "the completed line parses on the next read");
        assert!(!reset2);
        assert_eq!(
            offset,
            full.len() as u64,
            "offset advances past the full line"
        );
    }

    /// A missing file resets: `(empty, reset=true)` and `offset` set to 0.
    #[test]
    fn read_since_missing_file_resets() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("nonexistent.jsonl");
        let mut offset = 42u64;
        let (entries, reset) = read_since(&log, &mut offset).unwrap();
        assert!(entries.is_empty());
        assert!(reset, "missing file is a reset");
        assert_eq!(offset, 0, "offset resets to 0 on a missing file");
    }

    /// End-to-end rotation ↔ tail: crossing the cap rotates the file, and the next
    /// `read_since` sees `len < offset` and resets (commit 1 ↔ commit 2 seam).
    #[test]
    fn rotation_then_read_since_resets() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        for _ in 0..6 {
            append(&log, &sample_entry()).unwrap();
        }
        let mut offset = 0u64;
        let _ = read_since(&log, &mut offset).unwrap();
        assert!(offset > 0);

        // Rotate over a tiny injected cap, then recreate a fresh, smaller log.
        rotate_if_oversized_with_max(&log, 100);
        assert!(dir.path().join("log.jsonl.1").exists());
        append(&log, &sample_entry()).unwrap();

        let (entries, reset) = read_since(&log, &mut offset).unwrap();
        assert!(reset, "post-rotation file is shorter → reset");
        assert_eq!(entries.len(), 1, "view replaced with post-rotation entries");
    }
}
