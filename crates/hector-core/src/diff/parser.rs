use anyhow::{anyhow, Result};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: PathBuf,
}

/// Parse a unified diff string into a list of changed files.
pub fn parse_unified(input: &str) -> Result<Vec<ChangedFile>> {
    let mut files: Vec<ChangedFile> = Vec::new();
    let mut current: Option<ChangedFile> = None;

    for raw in input.lines() {
        if let Some(path) = raw.strip_prefix("+++ b/") {
            // A2: POSIX `diff -u` appends `\t<timestamp>` to header paths.
            // Split at the first tab and discard the timestamp segment before
            // any further processing.
            let path = path.split('\t').next().unwrap_or(path);
            // P2-10: tolerate lone trailing `\r` (CRLF-terminated lines passed
            // through `str::lines()` are already stripped, but defense in depth
            // protects us if iteration semantics ever change).
            let path = path.trim_end_matches('\r');
            // P0-4: reject paths that would escape the workspace. The diff
            // parser feeds these straight into the semantic context reader
            // and script-engine working dirs; an unchecked `+++ b/../../etc/passwd`
            // or `+++ b//etc/passwd` is direct exfil.
            if path.is_empty() {
                return Err(anyhow!("diff contains empty `+++ b/` path"));
            }
            if path.starts_with('/') {
                return Err(anyhow!("diff contains absolute path: {path}"));
            }
            let pb = PathBuf::from(path);
            if pb
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(anyhow!("diff contains path traversal: {path}"));
            }
            if let Some(f) = current.take() {
                files.push(f);
            }
            current = Some(ChangedFile { path: pb });
        }
        // All other lines (--- headers, @@ hunk headers, content lines) are
        // ignored — under D1=A, hector check evaluates the whole post-edit
        // file, so per-line tracking is not needed.
    }
    if let Some(f) = current {
        files.push(f);
    }
    Ok(files)
}
