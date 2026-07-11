//! Per-language import resolvers.

use std::path::{Path, PathBuf};

/// Resolve a raw import source string to an absolute file path on disk.
///
/// Returns `None` for anything that isn't a project-internal file: external
/// packages, stdlib, bare specifiers with no resolvable target. Unresolved
/// imports are dropped from the graph — they are not architectural edges.
///
/// Resolution is best-effort and conservative: a false drop is acceptable,
/// a false Block is not. The resolver never blocks.
pub trait Resolver: Send + Sync {
    fn resolve(&self, spec: &str, importer: &Path, root: &Path) -> Option<PathBuf>;
}

/// Shared helper: given a base path, try the common TS/JS extensions and
/// `/index.*` barrel forms. Returns the first existing file.
pub fn try_extensions(base: &Path) -> Option<PathBuf> {
    let base = normalize_path(base);
    let suffixes: [&str; 16] = [
        "",
        ".ts",
        ".tsx",
        ".mts",
        ".cts",
        ".js",
        ".jsx",
        ".mjs",
        ".cjs",
        ".d.ts",
        "/index.ts",
        "/index.tsx",
        "/index.js",
        "/index.jsx",
        "/index.mjs",
        "/index.cjs",
    ];
    for suffix in suffixes {
        let candidate = if suffix.is_empty() {
            base.clone()
        } else if let Some(stripped) = suffix.strip_prefix('/') {
            base.join(stripped)
        } else {
            PathBuf::from(format!("{}{}", base.display(), suffix))
        };
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Remove `.` and `..` segments from a path without touching the filesystem.
///
/// This keeps the result inside the original logical tree, unlike
/// `canonicalize` which resolves symlinks and may cross filesystem boundaries.
#[allow(clippy::path_buf_push_overwrite)]
pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Prefix(p) => out.push(p.as_os_str()),
            std::path::Component::RootDir => out.push("/"),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if let Some(parent) = out.parent() {
                    out = parent.to_path_buf();
                } else {
                    out.push("..");
                }
            }
            std::path::Component::Normal(p) => out.push(p),
        }
    }
    out
}
