use anyhow::Result;
use hector_core::session_state::{EditRecord, SessionState};
use std::path::Path;

pub fn record(dir: &Path, file: &Path, diff: &str, session_id: Option<String>) -> Result<i32> {
    let state_path = dir.join(".hector/session.json");
    let mut state = if state_path.exists() {
        SessionState::load(&state_path)?
    } else {
        let id =
            session_id.unwrap_or_else(|| format!("session-{}", chrono::Utc::now().timestamp()));
        SessionState::new(id)
    };
    state.append(EditRecord {
        file: file.display().to_string(),
        diff: diff.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    });
    state.save(&state_path)?;
    Ok(0)
}
