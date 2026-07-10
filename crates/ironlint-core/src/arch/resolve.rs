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
            base.to_path_buf()
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
