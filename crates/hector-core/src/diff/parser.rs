use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// The kind of change a diff hunk represents. Lets callers distinguish
/// deletions from additions and modifications without re-parsing the header
/// pair.
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
    /// What kind of change this entry represents.
    pub op: ChangeOp,
}

/// Validate a path segment extracted from a diff header. Rejects empty paths,
/// absolute paths, and paths containing `..` components — anything that could
/// escape the workspace. Shared by the `+++ b/` and `--- a/` parse arms.
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
/// Tracks both `---` and `+++` headers as a two-step state machine so
/// deletions (`+++ /dev/null`) surface as `ChangeOp::Deleted` rather than
/// being dropped.
pub fn parse_unified(input: &str) -> Result<Vec<ChangedFile>> {
    let mut files: Vec<ChangedFile> = Vec::new();
    let mut current: Option<ChangedFile> = None;
    // The `---` header sets this; the `+++` header consumes it to decide
    // whether the operation is Added or Modified. `None` means the `---` was
    // `/dev/null` (so the `+++` is an addition).
    let mut pending_minus: Option<PathBuf> = None;

    for raw in input.lines() {
        if let Some(minus) = raw.strip_prefix("--- ") {
            // Flush any in-progress file before starting a new header pair.
            if let Some(f) = current.take() {
                files.push(f);
            }
            // Strip the optional tab+timestamp suffix POSIX diff appends.
            let minus = minus.split('\t').next().unwrap_or(minus);
            let minus = minus.trim_end_matches('\r');
            // `--- /dev/null` means the new side is an addition; any other
            // non-`a/` form is unrecognised and treated as if we saw nothing.
            // Validate the `--- a/<path>` segment too — deletion diffs store
            // this path in pending_minus, which surfaces in
            // `ChangedFile { op: Deleted }` and telemetry.
            pending_minus = if let Some(path_str) = minus.strip_prefix("a/") {
                validate_path(path_str)?;
                Some(PathBuf::from(path_str))
            } else {
                None
            };
        } else if let Some(plus) = raw.strip_prefix("+++ ") {
            // Strip the optional tab+timestamp suffix POSIX diff appends.
            let plus = plus.split('\t').next().unwrap_or(plus);
            let plus = plus.trim_end_matches('\r');

            if plus == "/dev/null" {
                // Deletion — old side was a real file, new side is /dev/null.
                // `/dev/null` → `/dev/null` is nonsensical; only register when
                // we have a real pending `--- a/` path.
                if let Some(p) = pending_minus.take() {
                    current = Some(ChangedFile {
                        path: p,
                        op: ChangeOp::Deleted,
                    });
                }
            } else if let Some(p) = plus.strip_prefix("b/") {
                // Reject paths that would escape the workspace.
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
        // All other lines (@@ hunk headers, content lines) are ignored:
        // hector check evaluates the whole post-edit file, so per-line
        // tracking is not needed.
    }
    if let Some(f) = current {
        files.push(f);
    }
    Ok(files)
}
