//! Filesystem-only discovery of Git worktree identity for trust inheritance.
//!
//! Never shells out to `git` — its path and environment could be influenced by
//! the agent process. Nonstandard or unreadable Git metadata simply yields
//! `None` (no shared-worktree fallback); normal exact-path trust still works.

// This module is foundational: WorktreeScope is consumed by later trust-store
// tasks. Until those land, the crate build (not the test build) sees the
// pub(crate) API as unused; suppress dead-code rather than invent a consumer.
#![cfg_attr(not(test), allow(dead_code))]

use std::path::{Path, PathBuf};

pub(crate) struct WorktreeScope {
    /// Canonical absolute path of the Git *common* directory (the primary
    /// worktree's `.git` dir, shared by every linked worktree).
    pub(crate) common_dir: PathBuf,
    /// Canonical absolute path of the worktree root (the directory that
    /// contains the `.git` entry — dir for the primary, file for linked).
    pub(crate) worktree_root: PathBuf,
    /// The config path relative to `worktree_root`, `/`-separated, for store
    /// stability across platforms and across linked worktrees.
    pub(crate) config_rel: String,
}

impl WorktreeScope {
    pub(crate) fn discover(config_path: &Path) -> Option<Self> {
        let canon = config_path.canonicalize().ok()?;
        let worktree_root = find_git_root(canon.parent()?)?;
        let git_path = worktree_root.join(".git");
        let common_dir = resolve_common_dir(&git_path, &worktree_root)?;
        let config_rel = relative_path(&canon, &worktree_root)?;
        Some(Self {
            common_dir,
            worktree_root,
            config_rel,
        })
    }
}

/// Walk upward from `start` to the nearest ancestor containing a `.git` entry.
fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        if dir.join(".git").exists() {
            return dir.canonicalize().ok();
        }
        dir = dir.parent()?;
    }
}

/// Resolve the Git common directory from the `.git` entry at `git_path`.
/// `None` for symlinks, non-regular files, or malformed linked metadata.
fn resolve_common_dir(git_path: &Path, worktree_root: &Path) -> Option<PathBuf> {
    let meta = std::fs::symlink_metadata(git_path).ok()?;
    if meta.is_symlink() {
        return None; // never follow a symlinked .git
    }
    if meta.is_dir() {
        return git_path.canonicalize().ok(); // primary form
    }
    if meta.is_file() {
        return resolve_linked_common_dir(git_path, worktree_root);
    }
    None // FIFO/socket/device/other
}

/// Linked-worktree form: `.git` is a regular file with `gitdir: <linked-gitdir>`.
fn resolve_linked_common_dir(git_path: &Path, worktree_root: &Path) -> Option<PathBuf> {
    let body = std::fs::read_to_string(git_path).ok()?;
    let linked_gitdir = parse_gitdir_record(&body, worktree_root)?;
    let common = read_relative_target(&linked_gitdir.join("commondir"), &linked_gitdir)?;
    let reciprocal = read_target(&linked_gitdir.join("gitdir"))?;
    // The linked gitdir must live below <common>/worktrees/.
    let worktrees_base = common.join("worktrees");
    if !linked_gitdir.starts_with(&worktrees_base) {
        return None;
    }
    // The reciprocal gitdir must point back at this .git file.
    if reciprocal != git_path.canonicalize().ok()? {
        return None;
    }
    Some(common)
}

/// Parse a `gitdir: <path>` record, resolving relative paths against `root`.
fn parse_gitdir_record(body: &str, root: &Path) -> Option<PathBuf> {
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("gitdir:") {
            let raw = rest.trim();
            let p = if Path::new(raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                root.join(raw)
            };
            return p.canonicalize().ok();
        }
    }
    None
}

/// Read a file whose content is a path relative to `base`, resolved + canonicalized.
fn read_relative_target(p: &Path, base: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(p).ok()?;
    base.join(raw.trim()).canonicalize().ok()
}

/// Read a file whose content is an absolute path, canonicalized.
fn read_target(p: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(p).ok()?;
    PathBuf::from(raw.trim()).canonicalize().ok()
}

/// `canon` relative to `root`, `/`-separated. `None` if `canon` is not under `root`.
fn relative_path(canon: &Path, root: &Path) -> Option<String> {
    let rel = canon.strip_prefix(root).ok()?;
    let s = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn primary_git_dir_yields_scope() {
        // primary: .git is a directory; common_dir == that dir.
        let root = tempdir().unwrap();
        let git = root.path().join(".git");
        fs::create_dir_all(&git).unwrap();
        let cfg = root.path().join(".ironlint.yml");
        write(&cfg, "checks: {}\n");
        let scope = WorktreeScope::discover(&cfg).expect("primary .git dir is eligible");
        assert_eq!(scope.common_dir, git.canonicalize().unwrap());
        assert_eq!(scope.worktree_root, root.path().canonicalize().unwrap());
        assert_eq!(scope.config_rel, ".ironlint.yml");
    }

    #[test]
    fn linked_worktree_git_file_yields_shared_common_dir() {
        // Build a real linked layout by hand under tempdirs:
        //   primary/.git/                 <- common dir
        //   primary/.git/worktrees/linked/{commondir,gitdir}
        //   linked/.git                   <- file: gitdir: <primary>/.git/worktrees/linked
        let primary = tempdir().unwrap();
        let linked = tempdir().unwrap();
        let common = primary.path().join(".git");
        fs::create_dir_all(&common).unwrap();
        let linked_gitdir = common.join("worktrees").join("linked");
        fs::create_dir_all(&linked_gitdir).unwrap();
        // commondir is relative to the linked gitdir -> ../..  (= common)
        fs::write(linked_gitdir.join("commondir"), "../..\n").unwrap();
        // reciprocal gitdir is absolute -> the .git FILE at the linked root
        let linked_git_file = linked.path().join(".git");
        fs::write(
            &linked_git_file,
            format!("gitdir: {}\n", linked_gitdir.display()),
        )
        .unwrap();
        fs::write(
            linked_gitdir.join("gitdir"),
            format!("{}\n", linked_git_file.display()),
        )
        .unwrap();
        let cfg = linked.path().join(".ironlint.yml");
        write(&cfg, "checks: {}\n");
        let scope = WorktreeScope::discover(&cfg).expect("linked layout is eligible");
        assert_eq!(scope.common_dir, common.canonicalize().unwrap());
        assert_eq!(scope.worktree_root, linked.path().canonicalize().unwrap());
        assert_eq!(scope.config_rel, ".ironlint.yml");
    }

    #[test]
    fn missing_git_returns_none() {
        let root = tempdir().unwrap();
        let cfg = root.path().join("sub/.ironlint.yml");
        write(&cfg, "checks: {}\n");
        assert!(WorktreeScope::discover(&cfg).is_none());
    }

    #[test]
    fn symlink_git_file_is_refused() {
        // A symlinked .git must not be followed (security boundary).
        let root = tempdir().unwrap();
        let target = tempdir().unwrap();
        let cfg = root.path().join(".ironlint.yml");
        write(&cfg, "checks: {}\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            fs::create_dir_all(root.path().join(".git")).unwrap();
            // replace the dir with a symlink to elsewhere
            fs::remove_dir_all(root.path().join(".git")).unwrap();
            symlink(target.path(), root.path().join(".git")).unwrap();
            assert!(WorktreeScope::discover(&cfg).is_none());
        }
    }

    #[test]
    fn malformed_gitdir_record_returns_none() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join(".git")).unwrap(); // primary common
        let linked = tempdir().unwrap();
        let linked_git_file = linked.path().join(".git");
        fs::write(&linked_git_file, "not a gitdir record at all\n").unwrap();
        let cfg = linked.path().join(".ironlint.yml");
        write(&cfg, "checks: {}\n");
        assert!(WorktreeScope::discover(&cfg).is_none());
    }

    #[test]
    fn reciprocal_gitdir_mismatch_returns_none() {
        // gitdir file points somewhere else -> not a genuine linked worktree.
        let primary = tempdir().unwrap();
        let linked = tempdir().unwrap();
        let common = primary.path().join(".git");
        fs::create_dir_all(&common).unwrap();
        let linked_gitdir = common.join("worktrees").join("linked");
        fs::create_dir_all(&linked_gitdir).unwrap();
        fs::write(linked_gitdir.join("commondir"), "../..\n").unwrap();
        let linked_git_file = linked.path().join(".git");
        fs::write(
            &linked_git_file,
            format!("gitdir: {}\n", linked_gitdir.display()),
        )
        .unwrap();
        // reciprocal points at a DIFFERENT path than linked_git_file
        fs::write(linked_gitdir.join("gitdir"), "/somewhere/else/.git\n").unwrap();
        let cfg = linked.path().join(".ironlint.yml");
        write(&cfg, "checks: {}\n");
        assert!(WorktreeScope::discover(&cfg).is_none());
    }

    #[test]
    fn nested_config_relative_path_is_normalized() {
        // .ironlint.yml under sub/ -> config_rel "sub/.ironlint.yml"
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join(".git")).unwrap();
        let cfg = root.path().join("pkg/sub/.ironlint.yml");
        write(&cfg, "checks: {}\n");
        let scope = WorktreeScope::discover(&cfg).expect("nested config eligible");
        assert_eq!(scope.config_rel, "pkg/sub/.ironlint.yml");
    }
}
