use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Maximum number of edit records persisted to `session.json` (P2-18).
///
/// Long-running agent sessions previously grew the file without bound,
/// blowing up disk usage and turning the read-modify-write in
/// `hector session record` into O(N^2). On save we truncate to the most
/// recent `MAX_EDITS` entries, dropping the oldest. 1000 fits comfortably
/// in memory and on disk while still covering realistically long agent
/// sessions.
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
        // P2-2: treat a missing file as empty state rather than an IO error.
        // Adapters (Claude Code, opencode, future pre-commit) all want
        // "fresh-checkout means no recorded edits, just an empty session";
        // surfacing ENOENT through anyhow forced every caller to special-case
        // this. Mirror Baseline::load's NotFound handling.
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
        // P2-18: cap stored edits to the most recent `MAX_EDITS`, dropping
        // the oldest. Without this, long agent sessions accumulate every
        // recorded edit forever, growing session.json without bound and
        // making `hector session record` (which re-reads and re-writes the
        // whole file) effectively O(N^2). Truncating in-place keeps the
        // in-memory state consistent with what's on disk.
        if self.edits.len() > MAX_EDITS {
            let drop = self.edits.len() - MAX_EDITS;
            self.edits.drain(..drop);
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&temp, json)?;
        std::fs::rename(&temp, path)?;
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
