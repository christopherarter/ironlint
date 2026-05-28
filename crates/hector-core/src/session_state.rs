use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

/// Maximum number of edit records persisted to `session.json`.
///
/// Caps unbounded growth: without a ceiling a long-running agent session
/// accumulates every edit forever, blowing up disk usage and turning the
/// read-modify-write in `hector session record` into O(N^2). On save we
/// truncate to the most recent `MAX_EDITS`, dropping the oldest. 1000 fits
/// comfortably in memory and on disk while still covering realistically long
/// agent sessions.
pub const MAX_EDITS: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditRecord {
    pub file: String,
    pub diff: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub started_at: String,
    pub edits: Vec<EditRecord>,
}

impl SessionState {
    pub fn new(session_id: impl Into<String>) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            session_id: session_id.into(),
            started_at: now,
            edits: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        // Treat a missing file as empty state rather than an IO error.
        // Adapters all want "fresh-checkout means no recorded edits, just an
        // empty session"; surfacing ENOENT through anyhow would force every
        // caller to special-case it. Mirrors Baseline::load's NotFound
        // handling.
        if !path.exists() {
            return Ok(Self::new(""));
        }
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let s: Self = serde_json::from_str(&content)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(s)
    }

    pub fn save(&mut self, path: &Path) -> Result<()> {
        // Cap stored edits to the most recent `MAX_EDITS`, dropping the
        // oldest. Truncating in-place keeps the in-memory state consistent
        // with what's on disk.
        if self.edits.len() > MAX_EDITS {
            let drop = self.edits.len() - MAX_EDITS;
            self.edits.drain(..drop);
        }

        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(parent)?;

        // Place the temp alongside the target so `rename` stays on the
        // same filesystem (cross-fs rename is not atomic). Include the
        // PID to keep concurrent `save` invocations from clobbering each
        // other's temp files.
        let tmp_name = match path.file_name() {
            Some(n) => format!("{}.tmp.{}", n.to_string_lossy(), std::process::id()),
            None => format!("session.tmp.{}", std::process::id()),
        };
        let tmp_path = parent.join(tmp_name);

        let json = serde_json::to_string_pretty(self)?;
        {
            let mut f = std::fs::File::create(&tmp_path)?;
            f.write_all(json.as_bytes())?;
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

    pub fn clear(path: &Path) -> Result<()> {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn append(&mut self, edit: EditRecord) {
        self.edits.push(edit);
    }
}
