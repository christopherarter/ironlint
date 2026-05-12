use crate::config::ContextScope;
use anyhow::{anyhow, Result};
use std::path::Path;

/// Returns (primary, secondary) text for the LLM.
/// - `Diff`: primary = diff, no secondary.
/// - `File`: primary = file content, no secondary.
/// - `Repo`: primary = file content, secondary = "repo expansion deferred to 0.1c" stub.
pub fn expand_context(
    scope: ContextScope,
    diff: Option<&str>,
    file: Option<&Path>,
    _cwd: &Path,
) -> Result<(String, Option<String>)> {
    match scope {
        ContextScope::Diff => {
            let d = diff.ok_or_else(|| anyhow!("context: diff but no diff provided"))?;
            Ok((d.to_string(), None))
        }
        ContextScope::File => {
            let p = file.ok_or_else(|| anyhow!("context: file but no file provided"))?;
            let content = std::fs::read_to_string(p)?;
            Ok((content, None))
        }
        ContextScope::Repo => {
            let p = file.ok_or_else(|| anyhow!("context: repo requires a file as anchor"))?;
            let content = std::fs::read_to_string(p)?;
            // Full repo expansion (file + 2-hop imports) deferred. For 0.1b, repo == file with note.
            Ok((
                content,
                Some("(repo-context expansion deferred; using file content only)".to_string()),
            ))
        }
    }
}
