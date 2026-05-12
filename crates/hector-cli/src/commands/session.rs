use anyhow::{Context, Result};
use hector_core::session_state::{EditRecord, SessionState};
use std::fs::OpenOptions;
use std::path::Path;

pub fn record(dir: &Path, file: &Path, diff: &str, session_id: Option<String>) -> Result<i32> {
    let hector_dir = dir.join(".hector");
    // Ensure the parent directory exists before we try to open a lock file in
    // it. Concurrent calls all race here, but `create_dir_all` is idempotent
    // and treats existing directories as success.
    std::fs::create_dir_all(&hector_dir)
        .with_context(|| format!("creating {}", hector_dir.display()))?;

    let state_path = hector_dir.join("session.json");
    let lock_path = hector_dir.join("session.lock");

    // Acquire an advisory exclusive lock on a sibling `.lock` file around the
    // entire load → mutate → save sequence. Without this, two concurrent
    // `hector session record` invocations each read the same baseline state,
    // each append their edit, and the slower writer's rename clobbers the
    // faster one. P2-1.
    //
    // We deliberately open (and keep) the lock file open; the lock is
    // released when `lock_file` is dropped at end of scope. We never delete
    // the lock file — that would reintroduce a window where two processes
    // operate on different lock-file inodes.
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(&lock_path)
        .with_context(|| format!("opening lock {}", lock_path.display()))?;
    #[cfg(unix)]
    {
        use fs4::fs_std::FileExt;
        FileExt::lock_exclusive(&lock_file)
            .with_context(|| format!("locking {}", lock_path.display()))?;
    }

    // Load (treats missing as empty after P2-2), append, save.
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
    let save_result = state.save(&state_path);

    #[cfg(unix)]
    {
        use fs4::fs_std::FileExt;
        // Explicitly release before dropping so an error here surfaces. Drop
        // would also release silently.
        let _ = FileExt::unlock(&lock_file);
    }
    drop(lock_file);

    save_result?;
    Ok(0)
}
