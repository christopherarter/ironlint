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
            if let Some(f) = current.take() {
                files.push(f);
            }
            current = Some(ChangedFile {
                path: PathBuf::from(path),
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
