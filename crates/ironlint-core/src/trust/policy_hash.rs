use crate::adapter::sha256_digest_hex;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use super::worktree::WorktreeScope;

/// Feed one labeled blob into the hasher with length prefixes on both the
/// label and the content, so no two distinct (label, bytes) pairs can collide
/// by concatenation.
pub(super) fn hash_entry(hasher: &mut Sha256, label: &str, bytes: &[u8]) {
    hasher.update((label.len() as u64).to_le_bytes());
    hasher.update(label.as_bytes());
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Filesystem classification of a path in the scripts hash walk, computed via
/// `symlink_metadata` (which does **not** follow symlinks) rather than
/// `is_dir()`/`is_file()` (which do).
#[derive(Debug)]
pub(super) enum EntryKind {
    Dir,
    File,
    Missing,
}

/// Classify `path` for the scripts hash walk without ever following a
/// symlink. This walk runs on **unblessed** repo content before the trust
/// verdict is decided — it is the security boundary — so an unusual entry
/// is a hard error rather than a silent skip: a skipped file is un-hashed
/// and thus not trust-covered, which is worse than refusing to proceed.
/// Concretely this refuses:
/// - a symlink (a self-referencing symlink would otherwise recurse
///   indefinitely; a symlink to a FIFO would block a later `fs::read`
///   forever; a symlink to a device could read unbounded data), and
/// - any other non-regular file (FIFO, socket, device, ...).
///
/// A missing path is not an error — the caller decides what "missing"
/// means for its position in the walk (e.g. an absent scripts dir has
/// nothing to hash). A path whose parent isn't even a directory (e.g. a
/// plain file sits where `.ironlint/` should be) is treated the same as
/// missing: there is nothing there to hash, and this isn't the
/// symlink/non-regular-file class of problem the walk is guarding against.
pub(super) fn classify_entry(path: &Path) -> Result<EntryKind> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            let file_type = meta.file_type();
            if file_type.is_symlink() {
                anyhow::bail!(
                    "scripts dir contains a symlink ({}); refuse to hash — replace it with a regular file",
                    path.display()
                );
            }
            if file_type.is_dir() {
                return Ok(EntryKind::Dir);
            }
            if file_type.is_file() {
                return Ok(EntryKind::File);
            }
            anyhow::bail!(
                "scripts dir contains a non-regular file ({}); refuse to hash",
                path.display()
            );
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(EntryKind::Missing)
        }
        Err(e) => Err(e).with_context(|| format!("reading metadata for {}", path.display())),
    }
}

/// Recursively collect `(relative-path, bytes)` for every file under `dir`,
/// with `/`-separated relative paths for cross-platform determinism.
pub(super) fn collect_gate_files(dir: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    collect_into(dir, dir, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn collect_into(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        match classify_entry(&path)? {
            EntryKind::Dir => collect_into(root, &path, out)?,
            EntryKind::File => {
                let rel = path
                    .strip_prefix(root)
                    .expect("walked path must live under the scripts root")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                let bytes =
                    std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
                out.push((rel, bytes));
            }
            EntryKind::Missing => {
                // TOCTOU: read_dir just enumerated this entry, so it should
                // exist. If it vanished between listing and stat, fail
                // loudly rather than silently under-hashing the scripts dir.
                anyhow::bail!(
                    "scripts dir entry disappeared mid-walk ({})",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

/// Derive the sorted, deduped `.ironlint/scripts` directories participating
/// in an extends closure — one per distinct config-file directory in
/// `config_paths`. Shared by [`compute_hash`] (which folds these into the
/// hash) and [`blessed_summary`] (which enumerates them for display), so the
/// two can never disagree about which directories are in scope.
pub(super) fn closure_script_dirs(config_paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut script_dirs: Vec<PathBuf> = config_paths
        .iter()
        .map(|p| {
            p.parent()
                .unwrap_or_else(|| Path::new("."))
                .join(".ironlint")
                .join("scripts")
        })
        .collect();
    script_dirs.sort();
    script_dirs.dedup();
    script_dirs
}

/// Compute the trust hash of a config over its **entire `extends:` closure**.
///
/// Sha256 over every config file reachable from `config_path` (the root plus
/// every transitively-extended file) plus every file under each participating
/// config dir's `.ironlint/scripts/`. Returns `"sha256:<hex>"`.
///
/// Folding only the root config would let a blessed child that `extends:` a base
/// have that base — or the base's scripts — swapped under it without
/// invalidating the hash. Every blob is folded with [`hash_entry`]'s
/// length-prefixed framing and a label bound to the blob's identity (its
/// canonical config path, or its scripts dir + relative path), so neither
/// reordering nor relabeling can produce a collision. A no-`extends:` config
/// resolves to a one-element closure and keeps its prior behaviour: its own
/// edits, and edits to its own scripts, still revoke trust.
pub fn compute_hash(config_path: &Path) -> Result<String> {
    let mut hasher = Sha256::new();

    let config_paths = crate::config::extends::resolve_paths(config_path)
        .with_context(|| format!("resolving extends closure for {}", config_path.display()))?;

    // Fold each config file, keyed by its canonical path.
    for path in &config_paths {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        hash_entry(&mut hasher, &format!("config\0{}", path.display()), &bytes);
    }

    // Fold scripts under each distinct participating config dir's
    // `.ironlint/scripts/`. Dedup so a shared dir is never double-folded.
    let script_dirs = closure_script_dirs(&config_paths);

    for scripts_dir in &script_dirs {
        match classify_entry(scripts_dir)? {
            EntryKind::Dir => {
                for (rel, bytes) in collect_gate_files(scripts_dir)? {
                    hash_entry(
                        &mut hasher,
                        &format!("scripts\0{}\0{rel}", scripts_dir.display()),
                        &bytes,
                    );
                }
            }
            EntryKind::Missing => {}
            EntryKind::File => {
                anyhow::bail!(
                    "expected {} to be a directory (scripts dir)",
                    scripts_dir.display()
                );
            }
        }
    }

    Ok(sha256_digest_hex(&hasher.finalize()))
}

/// Compute the worktree-relative policy hash for `config_path` under `scope`.
///
/// Reuses the same extends-closure resolution, `.ironlint/scripts/`
/// enumeration, symlink refusal, sorting, and `hash_entry` framing as
/// [`compute_hash`] — the only semantic difference is the labels, which use
/// worktree-root-relative paths so the digest is stable across linked
/// worktrees. Any config or scripts dir that escapes `scope.worktree_root`
/// makes the policy ineligible (`Ok(None)`) rather than silently omitting a
/// file.
pub(super) fn compute_worktree_hash(
    config_path: &Path,
    scope: &WorktreeScope,
) -> Result<Option<String>> {
    let config_paths = crate::config::extends::resolve_paths(config_path)
        .with_context(|| format!("resolving extends closure for {}", config_path.display()))?;
    if !all_under_root(&config_paths, &scope.worktree_root) {
        return Ok(None);
    }
    let mut hasher = Sha256::new();
    for path in &config_paths {
        let rel = worktree_rel(path, &scope.worktree_root)?;
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        hash_entry(&mut hasher, &format!("config\0{rel}"), &bytes);
    }
    let script_dirs = closure_script_dirs(&config_paths);
    if !all_under_root(&script_dirs, &scope.worktree_root) {
        return Ok(None);
    }
    for scripts_dir in &script_dirs {
        match classify_entry(scripts_dir)? {
            EntryKind::Dir => {
                let dir_rel = worktree_rel(scripts_dir, &scope.worktree_root)?;
                for (rel, bytes) in collect_gate_files(scripts_dir)? {
                    hash_entry(&mut hasher, &format!("scripts\0{dir_rel}\0{rel}"), &bytes);
                }
            }
            EntryKind::Missing => {}
            EntryKind::File => {
                anyhow::bail!(
                    "expected {} to be a directory (scripts dir)",
                    scripts_dir.display()
                );
            }
        }
    }
    Ok(Some(sha256_digest_hex(&hasher.finalize())))
}

/// True iff every path in `paths` is under `root` (after canonicalization).
fn all_under_root(paths: &[PathBuf], root: &Path) -> bool {
    paths.iter().all(|p| p.strip_prefix(root).is_ok())
}

/// `canon` relative to `root`, `/`-separated. `Err` if not under `root`.
fn worktree_rel(canon: &Path, root: &Path) -> Result<String> {
    let rel = canon.strip_prefix(root).with_context(|| {
        format!(
            "{} escapes worktree root {}",
            canon.display(),
            root.display()
        )
    })?;
    Ok(rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/"))
}

#[cfg(test)]
#[path = "tests/policy_hash.rs"]
mod tests;
