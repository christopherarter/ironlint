use anyhow::{anyhow, Result};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub added_lines: Vec<u32>,
}

/// Parse a unified diff string into a list of changed files with added-line numbers.
pub fn parse_unified(input: &str) -> Result<Vec<ChangedFile>> {
    let mut files: Vec<ChangedFile> = Vec::new();
    let mut current: Option<ChangedFile> = None;
    let mut new_line_no: u32 = 0;

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
            current = Some(ChangedFile {
                path: pb,
                added_lines: Vec::new(),
            });
        } else if raw.starts_with("--- ") {
            // Old-file header; ignore.
        } else if let Some(rest) = raw.strip_prefix("@@ ") {
            // @@ -old_start,old_count +new_start,new_count @@
            let plus_idx = rest
                .find('+')
                .ok_or_else(|| anyhow!("malformed hunk header: {raw}"))?;
            let new_part = &rest[plus_idx + 1..];
            let comma_or_space = new_part
                .find([',', ' '])
                .ok_or_else(|| anyhow!("malformed hunk header: {raw}"))?;
            new_line_no = new_part[..comma_or_space]
                .parse::<u32>()
                .map_err(|e| anyhow!("hunk header parse: {e}"))?;
        } else if let Some(f) = current.as_mut() {
            if raw.strip_prefix('+').is_some() {
                if !raw.starts_with("+++") {
                    f.added_lines.push(new_line_no);
                    new_line_no += 1;
                }
            } else if !raw.starts_with('-') && !raw.starts_with("---") {
                // context or unchanged line advances new file line counter
                new_line_no += 1;
            }
        }
    }
    if let Some(f) = current {
        files.push(f);
    }
    Ok(files)
}
