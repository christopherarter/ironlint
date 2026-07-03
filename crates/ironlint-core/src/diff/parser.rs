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
    /// The file was renamed — `rename from <old>` + `rename to <new>`.
    Renamed,
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

/// Undo git's C-style quoting (`core.quotePath`). Git wraps paths in `"`
/// and escapes non-ASCII bytes as `\NNN` octal. We also handle the
/// standard C escapes `\n`, `\t`, `\r`, `\\`, `\"`. Returns the unquoted
/// UTF-8 string; returns an error if the quoted bytes are not valid UTF-8.
fn unquote_git_path(s: &str) -> Result<String> {
    if s.len() < 2 || !s.starts_with('"') || !s.ends_with('"') {
        return Ok(s.to_string());
    }
    let inner = &s[1..s.len() - 1];
    let mut out = Vec::new();
    let mut chars = inner.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\\' {
            // Non-escape characters are added as UTF-8 bytes. In a git-quoted
            // context, printable ASCII passes through literally.
            out.extend_from_slice(c.to_string().as_bytes());
            continue;
        }
        match chars.next() {
            None => return Err(anyhow!("truncated escape in quoted diff path")),
            Some('n') => out.push(b'\n'),
            Some('t') => out.push(b'\t'),
            Some('r') => out.push(b'\r'),
            Some('\\') => out.push(b'\\'),
            Some('"') => out.push(b'"'),
            Some(d) if d.is_ascii_digit() => {
                let mut octal = String::new();
                octal.push(d);
                for _ in 0..2 {
                    if let Some(&next) = chars.peek() {
                        if next.is_ascii_digit() {
                            octal.push(next);
                            chars.next();
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                let byte = u8::from_str_radix(&octal, 8).map_err(|e| {
                    anyhow!("invalid octal escape \\{octal} in quoted diff path: {e}")
                })?;
                out.push(byte);
            }
            Some(other) => return Err(anyhow!("unsupported escape \\{other} in quoted diff path")),
        }
    }

    String::from_utf8(out).map_err(|e| anyhow!("invalid UTF-8 in quoted diff path: {e}"))
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
            // A pending Renamed from a `rename to` line is dropped here: the
            // ---/+++ pair that follows describes the same file with content
            // changes, and the Modified entry is more specific.
            if let Some(f) = current.take() {
                if f.op != ChangeOp::Renamed {
                    files.push(f);
                }
            }
            let minus = minus.split('\t').next().unwrap_or(minus);
            let minus = minus.trim_end_matches('\r');
            let minus = unquote_git_path(minus)?;
            pending_minus = if let Some(path_str) = minus.strip_prefix("a/") {
                validate_path(path_str)?;
                Some(PathBuf::from(path_str))
            } else {
                None
            };
        } else if let Some(plus) = raw.strip_prefix("+++ ") {
            let plus = plus.split('\t').next().unwrap_or(plus);
            let plus = plus.trim_end_matches('\r');
            let plus = unquote_git_path(plus)?;

            if plus == "/dev/null" {
                if let Some(p) = pending_minus.take() {
                    current = Some(ChangedFile {
                        path: p,
                        op: ChangeOp::Deleted,
                    });
                }
            } else if let Some(p) = plus.strip_prefix("b/") {
                validate_path(p)?;
                let pb = PathBuf::from(p);
                let op = if pending_minus.take().is_some() {
                    ChangeOp::Modified
                } else {
                    ChangeOp::Added
                };
                current = Some(ChangedFile { path: pb, op });
            } else {
                return Err(anyhow!("unrecognized +++ header in diff: {plus}"));
            }
        } else if let Some(rename_to) = raw.strip_prefix("rename to ") {
            // Flush any in-progress file; a rename section starts a new entry.
            if let Some(f) = current.take() {
                files.push(f);
            }
            let rename_to = rename_to.split('\t').next().unwrap_or(rename_to);
            let rename_to = rename_to.trim_end_matches('\r');
            let rename_to = unquote_git_path(rename_to)?;
            validate_path(&rename_to)?;
            current = Some(ChangedFile {
                path: PathBuf::from(rename_to),
                op: ChangeOp::Renamed,
            });
            pending_minus = None;
        }
        // All other lines (@@ headers, content, "rename from", etc.) are ignored.
    }
    if let Some(f) = current {
        files.push(f);
    }
    Ok(files)
}
