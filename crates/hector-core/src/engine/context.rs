use crate::config::ContextScope;
use anyhow::{anyhow, Result};
use std::path::Path;

/// Returns (primary, secondary) text for the LLM.
/// - `Diff`: primary = diff, no secondary.
/// - `File`: primary = authoritative `content` when supplied, else the file
///   read from disk; no secondary.
/// - `Repo`: primary = same as `File`; secondary = a deferral note.
///
/// `content` is the authoritative bytes the caller already holds (a PreToolUse
/// `--content` payload or a successful diff-mode disk read). When present it is
/// preferred over a disk read, so `context: file`/`repo` rules evaluate the
/// proposed edit even before it lands on disk. When `None`, File/Repo read the
/// anchor file from disk as before.
pub fn expand_context(
    scope: ContextScope,
    diff: Option<&str>,
    file: Option<&Path>,
    content: Option<&str>,
    _cwd: &Path,
) -> Result<(String, Option<String>)> {
    match scope {
        ContextScope::Diff => {
            let d = diff.ok_or_else(|| anyhow!("context: diff but no diff provided"))?;
            Ok((d.to_string(), None))
        }
        ContextScope::File => {
            let body = file_body(content, file)?;
            Ok((body, None))
        }
        ContextScope::Repo => {
            let body = file_body(content, file)?;
            Ok((
                body,
                Some("(repo-context expansion deferred; using file content only)".to_string()),
            ))
        }
    }
}

/// Resolve File/Repo primary text: prefer authoritative `content`, else read
/// the anchor `file` from disk.
fn file_body(content: Option<&str>, file: Option<&Path>) -> Result<String> {
    if let Some(c) = content {
        return Ok(c.to_string());
    }
    let p = file.ok_or_else(|| anyhow!("context: file but no file or content provided"))?;
    Ok(std::fs::read_to_string(p)?)
}
