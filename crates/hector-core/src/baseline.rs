use crate::verdict::Violation;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Has the v1 baseline deprecation warning been emitted in this process?
///
/// Process-global so a single `hector check` invocation prints the
/// warning at most once even when several rule loops happen to load the
/// baseline. Across separate invocations the warning re-fires — that's
/// the intended nudge to run `hector baseline refresh`.
static LEGACY_WARNING_EMITTED: OnceLock<()> = OnceLock::new();

/// Per-entry baseline metadata.
///
/// An optional `body_sha256` sits alongside `line_sha256` so file-level
/// (`line: None`) violations participate in content-aware matching. Without
/// it, passthrough script output — the default — turns baseline into a
/// permanent per-file disable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_sha256: Option<String>,
}

/// On-disk baseline.
///
/// `entries` values are [`EntryMeta`] with optional `line_sha256` and
/// `body_sha256`; file-level violations store a normalized body hash and
/// replay requires both fingerprint AND body match.
///
/// Older on-disk formats load with a grace period: v2 (`Option<String>` line
/// checksum) matches on key+line only when `body_sha256` is missing; v1 (flat
/// fingerprint set) fires a one-time deprecation warning. Both upgrade on the
/// next `hector baseline refresh`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Baseline {
    pub entries: BTreeMap<String, EntryMeta>,
}

/// Summary of a [`Baseline::refresh`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshReport {
    /// Number of entries whose checksum was recomputed against current
    /// file content.
    pub refreshed: usize,
    /// Number of entries dropped because the baselined line is gone.
    pub dropped: usize,
}

/// Wire shape we accept on read. `Serialize` only emits v3 — every save
/// upgrades a v1/v2 file on the next write.
#[derive(Deserialize)]
#[serde(untagged)]
enum BaselineOnDisk {
    V3 {
        entries: BTreeMap<String, EntryMeta>,
    },
    V2 {
        entries: BTreeMap<String, Option<String>>,
    },
    V1 {
        fingerprints: Vec<String>,
    },
}

impl Baseline {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let parsed: BaselineOnDisk = serde_json::from_str(&content)?;
        match parsed {
            BaselineOnDisk::V3 { entries } => Ok(Self { entries }),
            BaselineOnDisk::V2 { entries } => {
                let upgraded = entries
                    .into_iter()
                    .map(|(k, line_sha256)| {
                        (
                            k,
                            EntryMeta {
                                line_sha256,
                                body_sha256: None,
                            },
                        )
                    })
                    .collect();
                Ok(Self { entries: upgraded })
            }
            BaselineOnDisk::V1 { fingerprints } => {
                Self::emit_legacy_warning(path);
                let entries = fingerprints
                    .into_iter()
                    .map(|fp| (fp, EntryMeta::default()))
                    .collect();
                Ok(Self { entries })
            }
        }
    }

    fn emit_legacy_warning(path: &Path) {
        if LEGACY_WARNING_EMITTED.set(()).is_ok() {
            eprintln!(
                "hector: warning — baseline at {} uses the legacy v1 format \
                 (no line_sha256). Run `hector baseline refresh` to upgrade. \
                 Old-format entries continue to suppress matching \
                 fingerprints during a grace period.",
                path.display()
            );
        }
    }

    /// Persist the baseline atomically.
    ///
    /// Serialize to a sibling temp file under the same parent directory,
    /// `fsync` the bytes, then `rename` onto the target — POSIX guarantees the
    /// rename is atomic on the same filesystem, so a crash mid-write never
    /// leaves a torn file: readers see either the full old file or the full
    /// new one.
    pub fn save(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;

        // Place the temp alongside the target so `rename` stays on the
        // same filesystem (cross-fs rename is not atomic). Include the
        // PID to keep concurrent `save` invocations from clobbering each
        // other's temp files.
        let tmp_name = match path.file_name() {
            Some(n) => format!("{}.tmp.{}", n.to_string_lossy(), std::process::id()),
            None => format!("baseline.tmp.{}", std::process::id()),
        };
        let tmp_path = parent.join(tmp_name);

        let payload = serde_json::to_string_pretty(self)?;
        {
            let mut f = std::fs::File::create(&tmp_path)?;
            f.write_all(payload.as_bytes())?;
            // sync_all flushes data + metadata so the rename below
            // promotes only fully-durable bytes onto the target.
            f.sync_all()?;
        }
        // If rename fails, do best-effort cleanup of the temp so we
        // don't litter the parent directory.
        if let Err(e) = std::fs::rename(&tmp_path, path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e.into());
        }
        Ok(())
    }

    /// Stable identity of a violation for baseline membership.
    ///
    /// JSON-encodes the `(rule_id, file, line)` tuple. A `::`-delimited string
    /// would collide when `::` appears in the rule_id or file path
    /// (`rule_id="a::b" file="c"` vs `rule_id="a" file="b::c"`); the JSON
    /// encoding is unambiguous and preserves the `Option<u32>` discriminant on
    /// `line`, so `line: None` and `line: Some(0)` stay distinct.
    pub fn fingerprint(v: &Violation) -> String {
        // Serializing a 3-tuple of primitives cannot fail; an `Err` here
        // would indicate a serde_json bug. Fall back to the legacy
        // format as a defensive last resort rather than panicking.
        serde_json::to_string(&(&v.rule_id, &v.file, &v.line))
            .unwrap_or_else(|_| format!("{}::{}::{}", v.rule_id, v.file, v.line.unwrap_or(0)))
    }

    /// SHA-256 of `line.trim_end()`. `trim_end` strips trailing `\r`,
    /// spaces and tabs so the checksum survives CRLF translation and
    /// editor whitespace normalization.
    pub fn line_checksum(line: &str) -> String {
        let mut h = Sha256::new();
        h.update(line.trim_end().as_bytes());
        format!("{:x}", h.finalize())
    }

    /// SHA-256 of a normalized message body.
    ///
    /// Normalization strips ISO-8601-shaped timestamps, ANSI color escapes,
    /// and per-line trailing whitespace. The normalized form is what gets
    /// hashed, so transient byproducts (line numbers in linter preambles,
    /// terminal color codes from interactive linters, scan timestamps) do
    /// not defeat matching.
    pub fn body_checksum(message: &str) -> String {
        let normalized = Self::normalize_body(message);
        let mut h = Sha256::new();
        h.update(normalized.as_bytes());
        format!("{:x}", h.finalize())
    }

    fn normalize_body(message: &str) -> String {
        // ANSI escape sequences: ESC [ ... letter
        // Implemented as a simple state machine rather than a regex to avoid
        // a new dep.
        let stripped_ansi = Self::strip_ansi(message);
        let stripped_ts = Self::strip_timestamps(&stripped_ansi);
        stripped_ts
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' && chars.peek() == Some(&'[') {
                chars.next();
                for inner in chars.by_ref() {
                    if inner.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn strip_timestamps(s: &str) -> String {
        // Match `\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}` and variants with
        // milliseconds + timezone. Implemented as a single pass.
        let bytes = s.as_bytes();
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        while i < bytes.len() {
            if Self::looks_like_iso8601(&bytes[i..]) {
                let mut j = i + 19; // YYYY-MM-DDTHH:MM:SS = 19 chars
                                    // Optional .fractional and timezone offset
                if j < bytes.len() && bytes[j] == b'.' {
                    j += 1;
                    while j < bytes.len() && bytes[j].is_ascii_digit() {
                        j += 1;
                    }
                }
                if j < bytes.len() && (bytes[j] == b'Z' || bytes[j] == b'+' || bytes[j] == b'-') {
                    j += 1;
                    // Skip up to 5 chars of offset (HH:MM or HHMM)
                    let end = (j + 5).min(bytes.len());
                    while j < end && (bytes[j].is_ascii_digit() || bytes[j] == b':') {
                        j += 1;
                    }
                }
                i = j;
            } else {
                // Advance by the byte length of the current UTF-8 char, not by
                // a single byte cast to char (which corrupts multi-byte sequences).
                let ch_len = s[i..].chars().next().map_or(1, |c| c.len_utf8());
                out.push_str(&s[i..i + ch_len]);
                i += ch_len;
            }
        }
        out
    }

    fn looks_like_iso8601(b: &[u8]) -> bool {
        if b.len() < 19 {
            return false;
        }
        b[0..4].iter().all(|c| c.is_ascii_digit())
            && b[4] == b'-'
            && b[5..7].iter().all(|c| c.is_ascii_digit())
            && b[7] == b'-'
            && b[8..10].iter().all(|c| c.is_ascii_digit())
            && b[10] == b'T'
            && b[11..13].iter().all(|c| c.is_ascii_digit())
            && b[13] == b':'
            && b[14..16].iter().all(|c| c.is_ascii_digit())
            && b[16] == b':'
            && b[17..19].iter().all(|c| c.is_ascii_digit())
    }

    /// 1-based line lookup. Returns `None` if `line == 0` or the line is
    /// past the end of `content`.
    fn line_at(file_content: &str, line: u32) -> Option<&str> {
        if line == 0 {
            return None;
        }
        file_content.lines().nth((line - 1) as usize)
    }

    pub fn add(&mut self, v: &Violation) {
        self.add_with_content(v, None);
    }

    /// Insert a violation. When `file_content` is `Some` and the
    /// violation has a `line`, capture the line text's SHA-256.
    /// When the violation has no `line`, capture a normalized body hash.
    pub fn add_with_content(&mut self, v: &Violation, file_content: Option<&str>) {
        let key = Self::fingerprint(v);
        let line_sha256 = v
            .line
            .and_then(|n| file_content.and_then(|c| Self::line_at(c, n)))
            .map(Self::line_checksum);
        let body_sha256 = if v.line.is_none() {
            Some(Self::body_checksum(&v.message))
        } else {
            None
        };
        self.entries.insert(
            key,
            EntryMeta {
                line_sha256,
                body_sha256,
            },
        );
    }

    pub fn contains(&self, v: &Violation) -> bool {
        self.contains_with_content(v, None)
    }

    /// Replay match. See [`Baseline`] type docs for the truth table.
    pub fn contains_with_content(&self, v: &Violation, file_content: Option<&str>) -> bool {
        let key = Self::fingerprint(v);
        let Some(meta) = self.entries.get(&key) else {
            return false;
        };
        if !Self::line_checksum_matches(meta.line_sha256.as_deref(), v.line, file_content) {
            return false;
        }
        Self::body_checksum_matches(meta.body_sha256.as_deref(), v.line, &v.message)
    }

    /// True when the stored line checksum matches the current line content.
    ///
    /// Cases:
    /// 1. No stored checksum (v1 grace period). Match.
    /// 2. Stored checksum but violation has no `line`. Match (body path handles it).
    /// 3. Stored checksum, line, and current content available. Hash and compare.
    ///    If the line is gone from the file, treat as still suppressed.
    /// 4. Stored checksum, line, but no current content. Conservatively match.
    fn line_checksum_matches(
        stored: Option<&str>,
        line: Option<u32>,
        content: Option<&str>,
    ) -> bool {
        let Some(expected) = stored else {
            return true;
        };
        let Some(n) = line else {
            return true;
        };
        let Some(c) = content else {
            return true;
        };
        match Self::line_at(c, n) {
            Some(text) => Self::line_checksum(text) == expected,
            None => true,
        }
    }

    fn body_checksum_matches(stored: Option<&str>, line: Option<u32>, message: &str) -> bool {
        // Grace period: v2 entries have no body_sha256. Match anything.
        let Some(expected) = stored else {
            return true;
        };
        // Line-bearing violations don't use body_sha256.
        if line.is_some() {
            return true;
        }
        Self::body_checksum(message) == *expected
    }

    /// Re-hash every entry against the current on-disk content of the
    /// file it points at. Drops entries whose line is no longer present
    /// in the file. File-level (`line: None`) entries pass through
    /// unchanged.
    pub fn refresh(&mut self, root: &Path) -> Result<RefreshReport> {
        let mut report = RefreshReport {
            refreshed: 0,
            dropped: 0,
        };
        let mut new_entries: BTreeMap<String, EntryMeta> = BTreeMap::new();

        for key in self.entries.keys() {
            match Self::refresh_one(key, root) {
                RefreshOutcome::Updated(checksum) => {
                    let prior_body = self.entries.get(key).and_then(|m| m.body_sha256.clone());
                    new_entries.insert(
                        key.clone(),
                        EntryMeta {
                            line_sha256: Some(checksum),
                            body_sha256: prior_body,
                        },
                    );
                    report.refreshed += 1;
                }
                RefreshOutcome::Dropped => {
                    report.dropped += 1;
                }
                RefreshOutcome::PassThrough => {
                    let prior = self.entries.get(key).cloned().unwrap_or_default();
                    new_entries.insert(key.clone(), prior);
                }
            }
        }
        self.entries = new_entries;
        Ok(report)
    }

    fn refresh_one(key: &str, root: &Path) -> RefreshOutcome {
        // Parse the key. Malformed keys pass through untouched: refresh
        // should never silently drop data it can't interpret.
        let Some((file_rel, maybe_line)) = Self::file_and_line_from_fingerprint(key) else {
            return RefreshOutcome::PassThrough;
        };
        let Some(line) = maybe_line else {
            // File-level entry — no line to hash.
            return RefreshOutcome::PassThrough;
        };
        let path = Self::join_rel(root, &file_rel);
        let Ok(content) = std::fs::read_to_string(&path) else {
            // File missing — keep the entry (don't penalize working
            // trees where the file was renamed).
            return RefreshOutcome::PassThrough;
        };
        if let Some(text) = Self::line_at(&content, line) {
            RefreshOutcome::Updated(Self::line_checksum(text))
        } else {
            eprintln!(
                "hector: refresh — dropping baseline entry {key}: line {line} no longer \
                 present in {}",
                path.display()
            );
            RefreshOutcome::Dropped
        }
    }

    /// Inverse of [`Baseline::fingerprint`]: pull back `(file, line)`
    /// from the JSON-encoded 3-tuple key. Returns `None` on a malformed
    /// key — refresh treats those as pass-through rather than dropping.
    fn file_and_line_from_fingerprint(key: &str) -> Option<(String, Option<u32>)> {
        let (_rule_id, file, line): (String, String, Option<u32>) =
            serde_json::from_str(key).ok()?;
        Some((file, line))
    }

    fn join_rel(root: &Path, rel: &str) -> PathBuf {
        let p = Path::new(rel);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        }
    }
}

enum RefreshOutcome {
    Updated(String),
    Dropped,
    PassThrough,
}
