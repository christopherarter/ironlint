use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

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
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let s: Self = serde_json::from_str(&content)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(s)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
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
