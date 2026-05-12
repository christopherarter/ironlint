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

/// On-disk baseline.
///
/// **v2 (E1):** `entries` maps the tuple-fingerprint key (see
/// [`Baseline::fingerprint`]) to an optional SHA-256 of the line content
/// at recording time. Replay matches when both the key and the checksum
/// match (or the stored checksum is absent — the grace-period behavior
/// for v1 baselines).
///
/// **v1 (pre-E1):** `fingerprints` is a flat set of tuple-fingerprint
/// strings. Loaded with a one-time deprecation warning; every entry is
/// treated as "always match" so existing baselines keep working. Run
/// `hector baseline refresh` to upgrade in place.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Baseline {
    pub entries: BTreeMap<String, Option<String>>,
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

/// Wire shape we accept on read. `Serialize` only emits v2 — every save
/// upgrades a v1 file on the next write.
#[derive(Deserialize)]
#[serde(untagged)]
enum BaselineOnDisk {
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
            BaselineOnDisk::V2 { entries } => Ok(Self { entries }),
            BaselineOnDisk::V1 { fingerprints } => {
                Self::emit_legacy_warning(path);
                let entries = fingerprints.into_iter().map(|fp| (fp, None)).collect();
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
    /// P2-5: the previous implementation used `std::fs::write` which
    /// `open(O_TRUNC) → write → close`s the target. A crash between
    /// truncate and the final write left a half-written or empty file
    /// that future loads couldn't parse. We now serialize to a sibling
    /// temp file under the same parent directory, `fsync` the bytes,
    /// and then `rename` onto the target — POSIX guarantees the rename
    /// is atomic on the same filesystem, so readers either see the full
    /// old file or the full new file, never a torn one.
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
    /// P1-4: the previous `"{rule_id}::{file}::{line}"` format collided
    /// when `::` appeared in either the rule_id or the file path
    /// (e.g. `rule_id="a::b" file="c"` vs `rule_id="a" file="b::c"`).
    /// JSON encoding of the tuple is unambiguous for every input and
    /// also preserves the `Option<u32>` discriminant on `line`, so
    /// `line: None` and `line: Some(0)` no longer collapse to the same
    /// fingerprint.
    pub fn fingerprint(v: &Violation) -> String {
        // Serializing a 3-tuple of primitives cannot fail; an `Err` here
        // would indicate a serde_json bug. Fall back to the legacy
        // format as a defensive last resort rather than panicking.
        serde_json::to_string(&(&v.rule_id, &v.file, &v.line))
            .unwrap_or_else(|_| format!("{}::{}::{}", v.rule_id, v.file, v.line.unwrap_or(0)))
    }

    /// SHA-256 of `line.trim_end()`. `trim_end` strips trailing `\r`,
    /// spaces and tabs so the checksum survives CRLF translation and
    /// editor whitespace normalization. Spec §E1 step 2 verbatim.
    pub fn line_checksum(line: &str) -> String {
        let mut h = Sha256::new();
        h.update(line.trim_end().as_bytes());
        format!("{:x}", h.finalize())
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
    pub fn add_with_content(&mut self, v: &Violation, file_content: Option<&str>) {
        let key = Self::fingerprint(v);
        let checksum = v
            .line
            .and_then(|n| file_content.and_then(|c| Self::line_at(c, n)))
            .map(Self::line_checksum);
        self.entries.insert(key, checksum);
    }

    pub fn contains(&self, v: &Violation) -> bool {
        self.contains_with_content(v, None)
    }

    /// Replay match. See [`Baseline`] type docs for the truth table.
    pub fn contains_with_content(&self, v: &Violation, file_content: Option<&str>) -> bool {
        let key = Self::fingerprint(v);
        let Some(stored) = self.entries.get(&key) else {
            return false;
        };
        Self::checksum_matches(stored.as_deref(), v.line, file_content)
    }

    /// True when the stored checksum matches the current line content.
    ///
    /// Extracted from `contains_with_content` to keep that method below
    /// the project's cognitive-complexity cap. Four cases:
    ///
    /// 1. No stored checksum (v1 grace period or file-level add). Match.
    /// 2. Stored checksum but violation has no `line`. Match.
    /// 3. Stored checksum, line, and current content available. Hash
    ///    the current line and compare. If the line is gone from the
    ///    file, treat as still suppressed — the violation cannot recur
    ///    on a line that no longer exists.
    /// 4. Stored checksum, line, but no current content (library caller
    ///    opted out). Conservatively match — preserves the pre-E1
    ///    behavior for that code path.
    fn checksum_matches(stored: Option<&str>, line: Option<u32>, content: Option<&str>) -> bool {
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

    /// Re-hash every entry against the current on-disk content of the
    /// file it points at. Drops entries whose line is no longer present
    /// in the file. File-level (`line: None`) entries pass through
    /// unchanged.
    pub fn refresh(&mut self, root: &Path) -> Result<RefreshReport> {
        let mut report = RefreshReport {
            refreshed: 0,
            dropped: 0,
        };
        let mut new_entries: BTreeMap<String, Option<String>> = BTreeMap::new();

        for key in self.entries.keys() {
            match Self::refresh_one(key, root) {
                RefreshOutcome::Updated(checksum) => {
                    new_entries.insert(key.clone(), Some(checksum));
                    report.refreshed += 1;
                }
                RefreshOutcome::Dropped => {
                    report.dropped += 1;
                }
                RefreshOutcome::PassThrough => {
                    new_entries.insert(key.clone(), None);
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
        match Self::line_at(&content, line) {
            Some(text) => RefreshOutcome::Updated(Self::line_checksum(text)),
            None => {
                eprintln!(
                    "hector: refresh — dropping baseline entry {key}: line {line} no longer \
                     present in {}",
                    path.display()
                );
                RefreshOutcome::Dropped
            }
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
