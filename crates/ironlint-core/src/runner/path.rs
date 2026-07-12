use std::path::{Path, PathBuf};

/// Canonicalize `path` if it exists; otherwise walk up to the deepest existing
/// ancestor, canonicalize that, and re-append the missing tail. `None` only if
/// no ancestor exists.
///
/// Needed for PreToolUse `--content`: the proposed edit targets a path that may
/// not exist on disk yet. Plain `canonicalize` fails, but the parent typically
/// resolves, and macOS's `/var → /private/var` symlink means the parent's
/// canonical form differs from its literal form. Resolving through the parent
/// produces a path that `strip_prefix(config_dir_canon)` can match.
pub(crate) fn canonicalize_through_parent(path: &Path) -> Option<PathBuf> {
    if let Ok(c) = path.canonicalize() {
        return Some(c);
    }
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path.to_path_buf();
    while let Some(name) = cursor.file_name() {
        suffix.push(name.to_os_string());
        if !cursor.pop() {
            break;
        }
        if let Ok(c) = cursor.canonicalize() {
            let mut out = c;
            for seg in suffix.into_iter().rev() {
                out.push(seg);
            }
            return Some(out);
        }
    }
    None
}

/// Resolve `path` to a `config_dir`-relative form for scope matching, falling
/// back to the canonical absolute path when the input resolves outside the
/// config dir (bare-pattern globs still match absolute paths via their
/// `**/<pattern>` form).
pub(crate) fn relativize(path: &Path, root: &Path) -> PathBuf {
    let canon_path = canonicalize_through_parent(path).unwrap_or_else(|| PathBuf::from(path));
    let canon_root = root.canonicalize().unwrap_or_else(|_| PathBuf::from(root));
    canon_path
        .strip_prefix(&canon_root)
        .map(PathBuf::from)
        .unwrap_or(canon_path)
}
