use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::policy_hash::{
    classify_entry, closure_script_dirs, collect_gate_files, compute_hash, compute_worktree_hash,
    EntryKind,
};
use super::worktree::WorktreeScope;

/// A read-only, human-facing enumeration of exactly what trust covers.
///
/// Covers the digest itself, the number of resolved checks, and every file
/// under `.ironlint/scripts/` folded into it. `compute_hash` retains no file
/// list of its own (it only ever returns the final digest), so this is a
/// fresh, faithful re-walk via the same helpers — not a cache of anything
/// `compute_hash` remembers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlessedSummary {
    /// The config path as passed to [`blessed_summary`].
    pub config_path: PathBuf,
    /// The authoritative digest, `"sha256:<hex>"`, identical to what
    /// [`compute_hash`] would return for the same `config_path`.
    pub config_hash: String,
    /// Number of resolved checks (post-extends merge).
    pub checks: usize,
    /// Every file relative path under `.ironlint/scripts/`, sorted and deduped.
    pub scripts: Vec<String>,
    /// Human-readable trust scope: `"linked worktrees"` when the policy is
    /// eligible for worktree-family inheritance, else `"this config path"`.
    pub scope: String,
}

/// Enumerate what trust covers for `config_path`.
///
/// Reports the digest plus the resolved check count and the scripts under
/// `.ironlint/scripts/` folded into it. Read-only — never writes the store or
/// the filesystem; safe to call any time after a config parses (typically
/// right after a successful [`bless`]).
///
/// Faithful to the full trust surface [`compute_hash`] folds (config +
/// scripts) — a summary that silently omitted the scripts surface would
/// misrepresent what was actually blessed.
pub fn blessed_summary(config_path: &Path) -> Result<BlessedSummary> {
    let config_hash = compute_hash(config_path)?;

    let config_paths = crate::config::extends::resolve_paths(config_path)
        .with_context(|| format!("resolving extends closure for {}", config_path.display()))?;
    let script_dirs = closure_script_dirs(&config_paths);

    let mut scripts: Vec<String> = Vec::new();
    for dir in &script_dirs {
        match classify_entry(dir)? {
            EntryKind::Dir => {
                for (rel, _bytes) in collect_gate_files(dir)? {
                    scripts.push(rel);
                }
            }
            EntryKind::Missing => {}
            EntryKind::File => {
                anyhow::bail!("expected {} to be a directory (scripts dir)", dir.display());
            }
        }
    }
    scripts.sort();
    scripts.dedup();

    let merged = crate::config::extends::resolve(config_path)?;
    let checks = merged.checks.len();

    let scope = match WorktreeScope::discover(config_path) {
        Some(s) if policy_is_eligible(config_path, &s).unwrap_or(false) => {
            "linked worktrees".to_string()
        }
        _ => "this config path".to_string(),
    };

    Ok(BlessedSummary {
        config_path: config_path.to_path_buf(),
        config_hash,
        checks,
        scripts,
        scope,
    })
}

/// True iff the resolved extends closure + scripts dirs are all under `scope.worktree_root`.
fn policy_is_eligible(config_path: &Path, scope: &WorktreeScope) -> Result<bool> {
    match compute_worktree_hash(config_path, scope)? {
        Some(_) => Ok(true),
        None => Ok(false),
    }
}

#[cfg(test)]
#[path = "tests/summary.rs"]
mod tests;
