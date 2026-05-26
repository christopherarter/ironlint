use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// C3: the kind of change a diff hunk represents. Allows callers to
/// distinguish deletions from additions and modifications without
/// re-parsing the header pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChangeOp {
    /// The file is new — `--- /dev/null` + `+++ b/<path>`.
    Added,
    /// The file exists in both tree-sides — `--- a/<path>` + `+++ b/<path>`.
    Modified,
    /// The file was removed — `--- a/<path>` + `+++ /dev/null`.
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: PathBuf,
    /// C3: what kind of change this entry represents.
    pub op: ChangeOp,
}

/// P0-4: validate a path segment extracted from a diff header. Rejects
/// empty paths, absolute paths, and paths containing `..` components.
/// Extracted so both the `+++ b/` and `--- a/` parse arms can reuse it.
fn validate_path(path: &str) -> Result<()> {
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
    Ok(())
}

/// Parse a unified diff string into a list of changed files.
///
/// C3: tracks both `---` and `+++` headers as a two-step state machine so
/// deletions (`+++ /dev/null`) are represented as `ChangeOp::Deleted` instead
/// of being silently dropped.
pub fn parse_unified(input: &str) -> Result<Vec<ChangedFile>> {
    let mut files: Vec<ChangedFile> = Vec::new();
    let mut current: Option<ChangedFile> = None;
    // C3: the `---` header sets this; the `+++` header consumes it to decide
    // whether the operation is Added or Modified. `None` means the previous
    // `---` was `/dev/null` (so the `+++` is an addition).
    let mut pending_minus: Option<PathBuf> = None;

    for raw in input.lines() {
        if let Some(minus) = raw.strip_prefix("--- ") {
            // Flush any in-progress file before starting a new header pair.
            if let Some(f) = current.take() {
                files.push(f);
            }
            // A2: strip optional tab+timestamp suffix.
            let minus = minus.split('\t').next().unwrap_or(minus);
            let minus = minus.trim_end_matches('\r');
            // `--- /dev/null` means the new side is an addition; any other
            // non-`a/` form is unrecognised and treated as if we saw nothing.
            pending_minus = minus.strip_prefix("a/").map(PathBuf::from);
        } else if let Some(plus) = raw.strip_prefix("+++ ") {
            // A2: strip optional tab+timestamp suffix.
            let plus = plus.split('\t').next().unwrap_or(plus);
            let plus = plus.trim_end_matches('\r');

            if plus == "/dev/null" {
                // C3: deletion — old side was a real file, new side is /dev/null.
                // `/dev/null` → `/dev/null` is nonsensical; only register when
                // we have a real pending `--- a/` path.
                if let Some(p) = pending_minus.take() {
                    current = Some(ChangedFile {
                        path: p,
                        op: ChangeOp::Deleted,
                    });
                }
            } else if let Some(p) = plus.strip_prefix("b/") {
                // P0-4: reject paths that would escape the workspace.
                validate_path(p)?;
                let pb = PathBuf::from(p);
                // If we saw a `--- a/` header, this is a modification;
                // if `---` was `/dev/null` (pending_minus is None), it's an addition.
                let op = if pending_minus.take().is_some() {
                    ChangeOp::Modified
                } else {
                    ChangeOp::Added
                };
                current = Some(ChangedFile { path: pb, op });
            }
            // Any other `+++ ` form (e.g. unquoted or unusual) is ignored.
        }
        // All other lines (@@ hunk headers, content lines) are ignored —
        // under D1=A, hector check evaluates the whole post-edit file, so
        // per-line tracking is not needed.
    }
    if let Some(f) = current {
        files.push(f);
    }
    Ok(files)
}
