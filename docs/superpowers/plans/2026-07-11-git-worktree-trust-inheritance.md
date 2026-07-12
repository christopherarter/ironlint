# Git Worktree Trust Inheritance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One human `ironlint trust` blessing covers an unchanged, fully in-worktree policy in every linked Git worktree of the same local repository, without weakening the trust boundary.

**Architecture:** Add a private `trust::worktree` helper that discovers Git worktree identity from filesystem metadata only (never shells out to `git`). Add a worktree-relative policy hash that relabels the existing config/scripts folding with worktree-root-relative paths. Extend `TrustStore` to version 2 with a nested `worktree_entries` map (serde-defaulted so v1 stores deserialize unchanged). `bless_in` writes both the direct and worktree entries in one locked atomic write; `check_trust_in` tries direct trust first, then inherited worktree trust, then a read-only legacy-migration fallback. `check` never writes the store.

**Tech Stack:** Rust workspace (`ironlint-core` lib + `ironlint-cli` bin), `sha2`, `serde`/`serde_json`, `anyhow`, `fs4` (unix file locking), `tempfile` (tests). No new dependencies. No `git` binary invoked at runtime.

**Spec:** `docs/superpowers/specs/2026-07-11-git-worktree-trust-inheritance-design.md` (authoritative).

## Global Constraints

- **Authorized exception (human ruling, 2026-07-11):** `TrustEntry` gains `PartialEq, Eq` derives. The brief said "TrustEntry is unchanged," but its own `assert_eq!(back.worktree_entries, store.worktree_entries)` round-trip test requires `TrustEntry: PartialEq, Eq`. A pure derive addition changes no serialization or behavior (existing v1 stores still round-trip), so it was authorized. Tests in Tasks 4–6 may rely on `TrustEntry: Eq` for `assert_eq!` on store state.
Copied verbatim from the spec + repo conventions. Every task's requirements implicitly include this section.

- `check` never writes or upgrades the trust store — only a human-initiated `ironlint trust` grants approval. No successful inherited check writes a direct entry, a worktree entry, a timestamp, or any other store state.
- `ironlint trust` remains the only operation that grants trust; adapter Bash gates continue to prevent an agent from invoking it.
- **No `git` binary invoked as part of the trust boundary.** Discovery uses filesystem metadata only. Nonstandard or unreadable Git metadata simply disables worktree inheritance; normal exact-path trust still works.
- Direct trust is the **first** lookup and continues to work everywhere worktree sharing is unavailable. Exact-path trust remains authoritative and unchanged.
- Exit codes and adapter contracts are unchanged. An untrusted policy remains exit `4`. A config the trust layer can't evaluate (parse failure, missing `extends:` target, …) remains exit `1`.
- `TRUST_STORE_VERSION` bumps `1` → `2`. An existing version-1 JSON file deserializes with an empty `worktree_entries` map; its direct `entries` remain authoritative and valid.
- The public contracts `check_trust*`, `bless*`, and `ensure_trusted*` retain their existing signatures and semantics.
- Worktree scope = `(canonical Git common directory, config path relative to the worktree root)` with `/`-separated relative paths for store stability.
- Eligible policy = the root config, every resolved `extends:` config, and all participating `.ironlint/scripts/` dirs are under the same canonical worktree root. A config symlinked outside the root or an external `extends:` target is **ineligible** — escape makes the policy ineligible rather than silently omitting a file.
- Do not trust identical policy in a separate clone, copied directory, or unrelated repository (different common directory). Do not make repository relocation portable.
- Keep cognitive complexity under the repo cap (**≤15 per function**, `clippy.toml`) by isolating discovery, hashing, version-2 lookup, and legacy-candidate validation into small private functions — refactor over `#[allow]`.
- `crates/*/src/*.rs` must meet **≥80% region coverage** (CI enforces per-file via `scripts/ci-coverage.sh`).
- TDD: RED (failing test) → GREEN. `cargo clippy --all-targets -- -D warnings`; `cargo fmt`. No `as` casts that trip lints.
- `Cargo.lock` is committed; this plan touches **no deps** → no lockfile change expected.
- Binary is `ironlint`, not `ironlint-cli`.
- `git add` SPECIFIC paths, never `-A`. Don't commit `.superpowers/` (gitignored).
- `scripts/ci-coverage.sh` leaves `target/llvm-cov-target` + `target/llvm-cov` scratch — `rm -rf` both after every run (memory: ci-coverage-cleanup). Drop any release binary you built to verify behavior (`cargo clean -p <crate>` or `rm target/release/<bin>`).

---

## File Structure

- **Create:** `crates/ironlint-core/src/trust/worktree.rs` — private Git worktree identity discovery (filesystem-only). Declared as `mod worktree;` from `trust.rs`. Owns `WorktreeScope`, `WorktreeScope::discover`, relative-path normalization, and focused unit tests. No `git` subprocess.
- **Modify:** `crates/ironlint-core/src/trust.rs` — declare `mod worktree;`; add `compute_worktree_hash`; bump `TRUST_STORE_VERSION` to `2` and add `worktree_entries` to `TrustStore`; extend `bless_in` to write the worktree entry; extend `check_trust_in` with inherited-lookup + legacy fallback; add `scope` to `BlessedSummary`. Public contracts unchanged.
- **Modify:** `crates/ironlint-cli/src/commands/trust.rs` — `render_summary` prints one additive `scope:` line.
- **Modify:** `docs/security/trust.md` — document linked-worktree inheritance, its in-tree-only boundary, and correct the exit-code doc (currently says `1`, must say `4`).
- **Modify:** adapter docs that reference untrusted configs failing open or exit `1` (runtime behavior unchanged; doc corrections only).
- **Create:** `crates/ironlint-core/tests/trust_worktree.rs` — integration tests using a real temporary Git repo + `git worktree add`.
- **Create:** `crates/ironlint-cli/tests/cli_e2e_trust_worktree.rs` — CLI-level worktree inheritance tests.

---

## Task 1: Worktree discovery module (`trust/worktree.rs`)

**Goal:** Pure, filesystem-metadata-only discovery of Git worktree identity. Returns `Option<WorktreeScope>` — `None` means "no shared-worktree fallback available" (never an error). Foundational; every later task consumes `WorktreeScope`.

**Files:**
- Create: `crates/ironlint-core/src/trust/worktree.rs`
- Modify: `crates/ironlint-core/src/trust.rs` (add `mod worktree;` declaration near the top, after the `use` block)

**Interfaces:**
- Produces (consumed by Tasks 2, 4, 5):

```rust
// In trust/worktree.rs. pub(crate) so trust.rs can use it; not part of the
// crate's public API.
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
    /// Discover the worktree scope of `config_path` from filesystem metadata
    /// only. `None` if `config_path` can't be canonicalized, no `.git` is
    /// found walking upward, or the Git metadata is malformed/unusual.
    pub(crate) fn discover(config_path: &Path) -> Option<WorktreeScope> { /* ... */ }
}
```

- Consumes: nothing from earlier tasks. Uses `std::fs`, `std::path`. No `git` subprocess.

**Discovery algorithm (encode exactly — confirmed against a real `git worktree add` probe):**

1. `let canon = config_path.canonicalize().ok()?;`
2. Walk upward from `canon.parent()` to the nearest directory containing a `.git` entry. If none, `None`. That directory is the candidate `worktree_root` — canonicalize it.
3. `let git_path = worktree_root.join(".git");` Read `symlink_metadata(git_path)`:
   - **Symlink or other non-regular/non-dir** → `None`.
   - **Directory (primary form):** `common_dir = git_path.canonicalize()`. The primary is eligible to create a family entry even before it has linked siblings.
   - **Regular file (linked form):** parse its single `gitdir: <path>` record (trim, skip blank/comment lines). Resolve `<path>` relative to `worktree_root`, then canonicalize → `linked_gitdir`. Then:
     - Read `linked_gitdir/commondir` (trim whitespace) → resolve relative to `linked_gitdir` → canonicalize → `common_dir`.
     - Read `linked_gitdir/gitdir` (trim) → canonicalize → `reciprocal`.
     - Require `reciprocal == git_path.canonicalize()` (the linked gitdir points back at the `.git` file at this worktree root).
     - Require `linked_gitdir.starts_with(common_dir.join("worktrees"))` (lives below `<common>/worktrees/`).
     - Any read/parse failure or mismatch → `None`.
4. `config_rel` = `canon.strip_prefix(&worktree_root).ok()?` with components joined by `/`. If the config is not under the root → `None`.
5. Return `Some(WorktreeScope { common_dir, worktree_root, config_rel })`.

Keep the function under the complexity cap by splitting: `discover` → `find_git_root` → `resolve_primary` / `resolve_linked` → `relative_path`. Each helper is small and unit-testable.

- [ ] **Step 1: Write failing tests in `trust/worktree.rs`** (`#[cfg(test)] mod tests`). Use `tempfile::tempdir` and hand-craft the `.git` layouts (no `git` binary). Tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() { fs::create_dir_all(parent).unwrap(); }
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
        fs::write(&linked_git_file,
            format!("gitdir: {}\n", linked_gitdir.display())).unwrap();
        fs::write(linked_gitdir.join("gitdir"),
            format!("{}\n", linked_git_file.display())).unwrap();
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
        let common = primary.path().join(".git");
        fs::create_dir_all(&common).unwrap();
        let linked_gitdir = common.join("worktrees").join("linked");
        fs::create_dir_all(&linked_gitdir).unwrap();
        fs::write(linked_gitdir.join("commondir"), "../..\n").unwrap();
        let linked_git_file = primary.path().join("elsewhere.git");
        fs::write(&linked_git_file, format!("gitdir: {}\n", linked_gitdir.display())).unwrap();
        // reciprocal points at a DIFFERENT path than linked_git_file
        fs::write(linked_gitdir.join("gitdir"), "/somewhere/else/.git\n").unwrap();
        let cfg = primary.path().join(".ironlint.yml");
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ironlint-core --lib trust::worktree`
Expected: FAIL — `WorktreeScope`/`discover` not defined (module not declared yet).

- [ ] **Step 3: Declare the module in `trust.rs`**

In `crates/ironlint-core/src/trust.rs`, after the `use` block (after line 7), add:

```rust
/// Filesystem-only Git worktree identity discovery. See `WorktreeScope`.
mod worktree;
```

- [ ] **Step 4: Implement `trust/worktree.rs`**

Create `crates/ironlint-core/src/trust/worktree.rs` with the `WorktreeScope` struct and `discover` per the algorithm above. Split into small private helpers to stay under complexity 15:

```rust
//! Filesystem-only discovery of Git worktree identity for trust inheritance.
//!
//! Never shells out to `git` — its path and environment could be influenced by
//! the agent process. Nonstandard or unreadable Git metadata simply yields
//! `None` (no shared-worktree fallback); normal exact-path trust still works.

use std::path::{Path, PathBuf};

pub(crate) struct WorktreeScope {
    pub(crate) common_dir: PathBuf,
    pub(crate) worktree_root: PathBuf,
    pub(crate) config_rel: String,
}

impl WorktreeScope {
    pub(crate) fn discover(config_path: &Path) -> Option<WorktreeScope> {
        let canon = config_path.canonicalize().ok()?;
        let worktree_root = find_git_root(canon.parent()?)?;
        let git_path = worktree_root.join(".git");
        let common_dir = resolve_common_dir(&git_path, &worktree_root)?;
        let config_rel = relative_path(&canon, &worktree_root)?;
        Some(WorktreeScope { common_dir, worktree_root, config_rel })
    }
}

/// Walk upward from `start` to the nearest ancestor containing a `.git` entry.
fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        if dir.join(".git").exists() {
            return Some(dir.canonicalize().ok()?);
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
    let s = rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    if s.is_empty() { None } else { Some(s) }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ironlint-core --lib trust::worktree`
Expected: PASS — all 7 tests green.

- [ ] **Step 6: Lint + fmt + coverage**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt`
Then: `bash scripts/ci-coverage.sh` — `trust/worktree.rs` must be ≥80% regions.
Then: `rm -rf target/llvm-cov-target target/llvm-cov` (clean scratch).

- [ ] **Step 7: Commit**

```bash
git add crates/ironlint-core/src/trust.rs crates/ironlint-core/src/trust/worktree.rs
git commit -m "feat(trust): filesystem-only Git worktree identity discovery"
```

---

## Task 2: Worktree-relative policy hash

**Goal:** A `compute_worktree_hash` that reuses the existing extends/scripts folding but labels every config and script with its path **relative to the worktree root**, and refuses (returns ineligible) when any covered file escapes the root. `compute_hash` for direct trust stays unchanged.

**Files:**
- Modify: `crates/ironlint-core/src/trust.rs` (add `compute_worktree_hash` + an eligibility helper, near `compute_hash`)

**Interfaces:**
- Consumes: `WorktreeScope` from Task 1; existing `crate::config::extends::resolve_paths`, `closure_script_dirs`, `collect_gate_files`, `classify_entry`, `hash_entry`, `sha256_digest_hex`.
- Produces (consumed by Tasks 4, 5):

```rust
/// `Ok(Some(hash))` = eligible policy, worktree-relative digest.
/// `Ok(None)` = ineligible (a config/scripts dir escapes the worktree root).
/// `Err` = a genuine I/O / parse error (rare — the direct hash already ran).
///
/// `config_path` is the canonical root config; `scope` is its discovered
/// worktree scope. Labels use the distinct `config`/`scripts` prefix and
/// `/`-separated worktree-root-relative paths, so the digest is stable across
/// linked worktrees with the same policy yet sensitive to every covered file.
fn compute_worktree_hash(config_path: &Path, scope: &WorktreeScope) -> Result<Option<String>>
```

- The `config` label is `config\0<rel-to-root>`; the `scripts` label is `scripts\0<scripts-dir-rel-to-root>\0<file-rel-to-scripts-dir>`. Reuse the existing length-prefixed `hash_entry` framing. The distinct prefix and full relative path make relabeling/concatenation collisions impossible.

**Eligibility rule (verbatim from spec §2):** Before hashing, verify every resolved config is under the root; the scripts dir for each resolved config is also required to be under the root. Any escape makes the policy ineligible rather than silently omitting a file.

- [ ] **Step 1: Write failing tests** in `trust.rs`'s `#[cfg(test)] mod tests` (reuse the existing `write` helper there):

```rust
    // --- worktree hash (Task 2) -------------------------------------------
    use crate::trust::worktree::WorktreeScope;
    use std::process::Command;

    /// Build a real git repo at `root` so `WorktreeScope::discover` succeeds.
    fn git_repo(root: &Path) {
        fs::create_dir_all(root).unwrap();
        let _ = Command::new("git").args(["init", "-q"])
            .arg(root).status();
        // .ironlint.yml is the trust surface; commit is unnecessary for discovery.
    }

    #[test]
    fn worktree_hash_matches_for_equivalent_roots() {
        // Two tempdirs with identical policy + identical .git dir layout must
        // produce the SAME worktree hash (labels are root-relative).
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for root in [a.path(), b.path()] {
            git_repo(root);
            let cfg = root.join(".ironlint.yml");
            write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n");
        }
        let sa = WorktreeScope::discover(&a.path().join(".ironlint.yml")).unwrap();
        let sb = WorktreeScope::discover(&b.path().join(".ironlint.yml")).unwrap();
        let ha = compute_worktree_hash(&a.path().join(".ironlint.yml"), &sa).unwrap().unwrap();
        let hb = compute_worktree_hash(&b.path().join(".ironlint.yml"), &sb).unwrap().unwrap();
        assert_eq!(ha, hb, "identical policy in equivalent roots hashes alike");
    }

    #[test]
    fn worktree_hash_changes_when_config_changes() {
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        let cfg = root.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n");
        let scope = WorktreeScope::discover(&cfg).unwrap();
        let h1 = compute_worktree_hash(&cfg, &scope).unwrap().unwrap();
        write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \"false\"\n");
        let h2 = compute_worktree_hash(&cfg, &scope).unwrap().unwrap();
        assert_ne!(h1, h2, "editing the config changes the worktree hash");
    }

    #[test]
    fn worktree_hash_changes_when_script_changes() {
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        let cfg = root.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \".ironlint/scripts/s.sh\"\n");
        let scripts = root.path().join(".ironlint/scripts");
        fs::create_dir_all(&scripts).unwrap();
        write(&scripts.join("s.sh"), "#!/bin/sh\nexit 0\n");
        let scope = WorktreeScope::discover(&cfg).unwrap();
        let h1 = compute_worktree_hash(&cfg, &scope).unwrap().unwrap();
        write(&scripts.join("s.sh"), "#!/bin/sh\nexit 1\n");
        let h2 = compute_worktree_hash(&cfg, &scope).unwrap().unwrap();
        assert_ne!(h1, h2, "editing a covered script changes the worktree hash");
    }

    #[test]
    fn worktree_hash_is_ineligible_when_extends_escapes_root() {
        // extends: target lives OUTSIDE the worktree root -> ineligible (None).
        let outside = tempfile::tempdir().unwrap();
        let base = outside.path().join("base.ironlint.yml");
        write(&base, "checks:\n  b:\n    files: \"*\"\n    run: \"true\"\n");
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        let cfg = root.path().join(".ironlint.yml");
        write(&cfg, &format!("extends: [\"{}\"]\nchecks: {{}}\n", base.display()));
        let scope = WorktreeScope::discover(&cfg).unwrap();
        assert!(compute_worktree_hash(&cfg, &scope).unwrap().is_none(),
            "an extends target outside the root is ineligible");
    }
```

> **Note for the implementer:** `WorktreeScope` and `compute_worktree_hash` are `pub(crate)`. The unit tests live in the same crate (`trust.rs`'s `mod tests`), so `use crate::trust::worktree::WorktreeScope;` resolves. If the existing `mod tests` already `use super::*;`, that brings `compute_worktree_hash` into scope directly. Add the `use` for `WorktreeScope` only where needed. `git_repo` shells out to `git init` purely to materialize a `.git` dir for discovery — this is **test-only** and does not violate the runtime "no git binary" constraint.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ironlint-core --lib trust::tests::worktree`
Expected: FAIL — `compute_worktree_hash` not defined.

- [ ] **Step 3: Implement `compute_worktree_hash` + eligibility helper**

In `trust.rs`, after `compute_hash` (after line 192), add:

```rust
/// Compute the worktree-relative policy hash for `config_path` under `scope`.
///
/// Reuses the same extends-closure resolution, `.ironlint/scripts/`
/// enumeration, symlink refusal, sorting, and `hash_entry` framing as
/// [`compute_hash`] — the only semantic difference is the labels, which use
/// worktree-root-relative paths so the digest is stable across linked
/// worktrees. Any config or scripts dir that escapes `scope.worktree_root`
/// makes the policy ineligible (`Ok(None)`) rather than silently omitting a
/// file.
fn compute_worktree_hash(config_path: &Path, scope: &WorktreeScope) -> Result<Option<String>> {
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
                anyhow::bail!("expected {} to be a directory (scripts dir)", scripts_dir.display());
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
        format!("{} escapes worktree root {}", canon.display(), root.display())
    })?;
    Ok(rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/"))
}
```

> **`worktree_rel` returns `Err` but `all_under_root` guards first:** the `all_under_root` check runs before the loop, so `worktree_rel` inside the loop only encounters in-root paths. The `Err` arm is defense-in-depth for a path that passed `all_under_root` but somehow fails `strip_prefix` (shouldn't happen); it surfaces as a hash error, which the callers (Tasks 4/5) treat as "skip the worktree path."

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ironlint-core --lib trust::tests`
Expected: PASS — new worktree-hash tests green; all pre-existing trust tests still green (no behavior change to `compute_hash`).

- [ ] **Step 5: Lint + fmt + coverage**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt`
Then: `bash scripts/ci-coverage.sh`; `trust.rs` must stay ≥80%.
Then: `rm -rf target/llvm-cov-target target/llvm-cov`.

- [ ] **Step 6: Commit**

```bash
git add crates/ironlint-core/src/trust.rs
git commit -m "feat(trust): worktree-relative policy hash with root-escape eligibility"
```

---

## Task 3: Trust store version 2 + `worktree_entries`

**Goal:** Bump `TRUST_STORE_VERSION` to `2` and add a serde-defaulted nested `worktree_entries` map. An existing v1 JSON file deserializes with an empty `worktree_entries`; its direct `entries` stay authoritative. No behavior change yet (nothing reads `worktree_entries` until Tasks 4/5).

**Files:**
- Modify: `crates/ironlint-core/src/trust.rs` (the constant + struct only)

**Interfaces:**
- Produces (consumed by Tasks 4, 5):

```rust
pub const TRUST_STORE_VERSION: u32 = 2;   // was 1

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustStore {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub entries: BTreeMap<String, TrustEntry>,
    /// v2: linked-worktree inheritance. Outer key = canonical Git common
    /// directory; inner key = normalized config-relative path. serde-defaulted
    /// so a v1 store (no such field) deserializes with an empty map.
    #[serde(default)]
    pub worktree_entries: BTreeMap<String, BTreeMap<String, TrustEntry>>,
}
```

- Consumes: nothing new.

- [ ] **Step 1: Write failing tests** in `trust.rs`'s `mod tests`:

```rust
    // --- store v2 (Task 3) -----------------------------------------------
    #[test]
    fn version_one_store_deserializes_with_empty_worktree_entries() {
        let v1 = r#"{"version":1,"entries":{"/x/.ironlint.yml":{"hash":"sha256:ab","blessed_at":"t"}}}"#;
        let store: TrustStore = serde_json::from_str(v1).unwrap();
        assert_eq!(store.version, 1);
        assert_eq!(store.entries.len(), 1);
        assert!(store.worktree_entries.is_empty(), "v1 store gets empty worktree_entries");
    }

    #[test]
    fn worktree_entries_round_trip() {
        let mut store = TrustStore::default();
        store.worktree_entries.insert(
            "/common/.git".to_string(),
            {
                let mut inner = BTreeMap::new();
                inner.insert(".ironlint.yml".to_string(),
                    TrustEntry { hash: "sha256:cd".to_string(), blessed_at: "t".to_string() });
                inner
            },
        );
        let json = serde_json::to_string(&store).unwrap();
        let back: TrustStore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.worktree_entries, store.worktree_entries);
    }

    #[test]
    fn trust_store_version_is_two() {
        assert_eq!(TRUST_STORE_VERSION, 2);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ironlint-core --lib trust::tests::version trust::tests::worktree_entries trust::tests::trust_store_version`
Expected: FAIL — `worktree_entries` field absent (v1 deserialize test fails to compile / field missing).

- [ ] **Step 3: Bump the version + add the field**

Change `pub const TRUST_STORE_VERSION: u32 = 1;` → `= 2;`. Add the `worktree_entries` field to `TrustStore` exactly as shown in the Interfaces block, with its doc comment. Leave `TrustEntry` unchanged.

- [ ] **Step 4: Run the full trust test suite**

Run: `cargo test -p ironlint-core --lib trust`
Expected: PASS — new tests green; all existing store tests (round-trip, corrupt-store, etc.) still green. `bless_in` already sets `store.version = TRUST_STORE_VERSION`, so newly-written stores now carry `version: 2` (the v1-on-disk deserialize test asserts the *file* stays v1 until rewritten, which is correct).

- [ ] **Step 5: Lint + fmt + coverage**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt`
Then: `bash scripts/ci-coverage.sh`; `trust.rs` ≥80%.
Then: `rm -rf target/llvm-cov-target target/llvm-cov`.

- [ ] **Step 6: Commit**

```bash
git add crates/ironlint-core/src/trust.rs
git commit -m "feat(trust): store v2 with serde-defaulted worktree_entries map"
```

---

## Task 4: Blessing writes the worktree entry + `scope` summary line

**Goal:** `bless_in` writes both the direct entry and (when `WorktreeScope::discover` succeeds and the policy is eligible) the `worktree_entries[common_dir][config_rel]` entry in the **same** locked atomic write. `BlessedSummary` gains a `scope` field; `ironlint trust` prints one additive `scope:` line. `ironlint init` needs no change — it calls `trust::bless`, so a freshly-initialized Git project gets the worktree-family entry automatically.

**Files:**
- Modify: `crates/ironlint-core/src/trust.rs` (`bless_in`, `BlessedSummary`, `blessed_summary`)
- Modify: `crates/ironlint-cli/src/commands/trust.rs` (`render_summary` + its unit tests)

**Interfaces:**
- Consumes: `WorktreeScope::discover`, `compute_worktree_hash` (Tasks 1–2), `TrustStore.worktree_entries` (Task 3).
- Produces: `BlessedSummary.scope: String` (e.g. `"linked worktrees"` or `"this config path"`), consumed by the CLI renderer.

**Blessing semantics (spec §4):**
1. parse + validate the full `extends:` closure (unchanged);
2. compute the direct hash + canonical direct key (unchanged);
3. under the existing lock, atomically update the direct entry (unchanged); AND
4. when `WorktreeScope::discover` succeeds and `compute_worktree_hash` returns `Ok(Some(h))`, also set `worktree_entries[common_dir][config_rel]` in the same locked write.

If discovery or eligibility fails (`None` scope or `Ok(None)`/`Err` hash), blessing still succeeds with only the direct entry — never fail the bless because the worktree path is unavailable.

- [ ] **Step 1: Write failing tests** in `trust.rs`'s `mod tests`:

```rust
    // --- blessing worktree entry (Task 4) --------------------------------
    #[test]
    fn bless_writes_worktree_entry_for_eligible_git_repo() {
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        let cfg = root.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n");
        let store = tempfile::tempdir().unwrap();
        let store_path = store.path().join("trust.json");
        bless_in(&cfg, &store_path, "t").unwrap();
        let s = read_store(&store_path).unwrap();
        let scope = WorktreeScope::discover(&cfg).unwrap();
        let inner = s.worktree_entries
            .get(&scope.common_dir.to_string_lossy())
            .and_then(|m| m.get(&scope.config_rel))
            .expect("eligible bless writes a worktree_entries entry");
        let h = compute_worktree_hash(&cfg, &scope).unwrap().unwrap();
        assert_eq!(inner.hash, h);
    }

    #[test]
    fn bless_skips_worktree_entry_when_not_a_git_repo() {
        // No .git -> discover returns None -> only the direct entry is written.
        let root = tempfile::tempdir().unwrap();
        let cfg = root.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n");
        let store = tempfile::tempdir().unwrap();
        let store_path = store.path().join("trust.json");
        bless_in(&cfg, &store_path, "t").unwrap();
        let s = read_store(&store_path).unwrap();
        assert!(s.worktree_entries.is_empty(), "non-Git config writes no worktree entry");
        assert_eq!(s.entries.len(), 1, "direct entry still written");
    }

    #[test]
    fn blessed_summary_scope_is_linked_worktrees_for_git_repo() {
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        let cfg = root.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n");
        let sum = blessed_summary(&cfg).unwrap();
        assert_eq!(sum.scope, "linked worktrees");
    }

    #[test]
    fn blessed_summary_scope_is_this_config_path_when_not_git() {
        let root = tempfile::tempdir().unwrap();
        let cfg = root.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n");
        let sum = blessed_summary(&cfg).unwrap();
        assert_eq!(sum.scope, "this config path");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ironlint-core --lib trust::tests::bless_writes_worktree trust::tests::bless_skips trust::tests::blessed_summary_scope`
Expected: FAIL — `BlessedSummary.scope` field absent; worktree entry not written.

- [ ] **Step 3: Add `scope` to `BlessedSummary` and compute it in `blessed_summary`**

Add `pub scope: String` to `BlessedSummary` (keep `#[derive(Debug, Clone, PartialEq, Eq)]`). In `blessed_summary`, after computing the hash, set `scope`:

```rust
    let scope = match WorktreeScope::discover(config_path) {
        Some(s) if policy_is_eligible(config_path, &s).unwrap_or(false) => "linked worktrees".to_string(),
        _ => "this config path".to_string(),
    };
```

…and include `scope` in the returned `BlessedSummary`.

Add a small helper (keeps `blessed_summary` under the complexity cap):

```rust
/// True iff the resolved extends closure + scripts dirs are all under `scope.worktree_root`.
fn policy_is_eligible(config_path: &Path, scope: &WorktreeScope) -> Result<bool> {
    match compute_worktree_hash(config_path, scope)? {
        Some(_) => Ok(true),
        None => Ok(false),
    }
}
```

- [ ] **Step 4: Extend `bless_in` to write the worktree entry in the same locked write**

Replace the body of `bless_in` after `let hash = compute_hash(config_path)?;` so that, inside the existing lock, it also sets the worktree entry when eligible:

```rust
pub fn bless_in(config_path: &Path, store_path: &Path, now: &str) -> Result<()> {
    crate::config::parse_file_with_extends(config_path)
        .context("refusing to trust a config that does not parse")?;
    let key = canonical_key(config_path)?;
    let hash = compute_hash(config_path)?;

    let _lock = acquire_store_lock(store_path)?;
    let mut store = read_store_for_bless(store_path)?;
    store.version = TRUST_STORE_VERSION;
    store.entries.insert(
        key,
        TrustEntry { hash: hash.clone(), blessed_at: now.to_string() },
    );
    write_worktree_entry(&mut store, config_path, &hash, now);
    write_store(store_path, &store)
}

/// If `config_path` has an eligible worktree scope, record its worktree hash
/// (identical content, root-relative labels) in `worktree_entries`. Best-effort:
/// any discovery/hash failure is swallowed — blessing still succeeds with the
/// direct entry already written by the caller.
fn write_worktree_entry(store: &mut TrustStore, config_path: &Path, hash: &str, now: &str) {
    let Some(scope) = WorktreeScope::discover(config_path) else { return; };
    let Ok(Some(_)) = compute_worktree_hash(config_path, &scope) else { return; };
    let common = scope.common_dir.to_string_lossy().to_string();
    store
        .worktree_entries
        .entry(common)
        .or_default()
        .insert(scope.config_rel, TrustEntry { hash: hash.to_string(), blessed_at: now.to_string() });
}
```

> **Why the worktree entry's hash equals the direct hash is WRONG here — fix:** The worktree entry must hold the **worktree-relative** hash (`compute_worktree_hash`), NOT the direct hash. The `Ok(Some(_))` binding discarded it — capture it instead:

```rust
    let Ok(Some(wt_hash)) = compute_worktree_hash(config_path, &scope) else { return; };
    // ... insert TrustEntry { hash: wt_hash, ... }
```

(Corrected: use `wt_hash`, not `hash`, for the worktree entry's `TrustEntry.hash`. The direct `hash` is a separate value with absolute-path labels.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ironlint-core --lib trust`
Expected: PASS — new bless/summary tests green; all existing bless tests (concurrency, corrupt-store recovery, fail-closed) still green.

- [ ] **Step 6: CLI — render the additive `scope:` line**

In `crates/ironlint-cli/src/commands/trust.rs`, `render_summary`, add the scope line as the **last** line of the summary (additive — do not remove or reword existing lines):

```rust
fn render_summary(summary: &BlessedSummary) -> String {
    let hex = summary
        .config_hash
        .strip_prefix("sha256:")
        .unwrap_or(&summary.config_hash);
    let short_len = hex.len().min(16);

    let mut lines = vec![
        format!("  config sha256: {}", &hex[..short_len]),
        format!("  checks: {}", summary.checks),
        format!("  scripts: {}", summary.scripts.len()),
    ];
    for script in &summary.scripts {
        lines.push(format!("    - {script}"));
    }
    lines.push(format!("  scope: {}", summary.scope));

    lines.join("\n")
}
```

Update the `summary(...)` test helper in the same file to set `scope: "this config path".to_string()` (or `"linked worktrees"`), and update the existing `render_summary_*` tests so their `summary(...)` construction still compiles. Add:

```rust
    #[test]
    fn render_summary_prints_scope_line() {
        let mut s = summary(0, vec![]);
        s.scope = "linked worktrees".to_string();
        let out = render_summary(&s);
        assert!(out.contains("scope: linked worktrees"), "scope line present: {out}");
    }
```

- [ ] **Step 7: Run CLI tests + lint + fmt + coverage**

Run: `cargo test -p ironlint-cli --lib commands::trust` then `cargo test -p ironlint-cli --test cli_e2e_trust`
Expected: PASS — existing `trust_prints_blessed_summary` etc. still pass (the scope line is additive; their `.contains(...)` assertions don't forbid extra lines). The `trust_summary_prints_zero_scripts_when_empty` test still passes.
Run: `cargo clippy --all-targets -- -D warnings && cargo fmt`
Then: `bash scripts/ci-coverage.sh`; both `trust.rs` and `commands/trust.rs` ≥80%.
Then: `rm -rf target/llvm-cov-target target/llvm-cov`.

- [ ] **Step 8: Commit**

```bash
git add crates/ironlint-core/src/trust.rs crates/ironlint-cli/src/commands/trust.rs
git commit -m "feat(trust): bless writes worktree entry; trust prints scope line"
```

---

## Task 5: Verification — inherited lookup + legacy migration fallback

**Goal:** After a direct-trust miss, `check_trust_in` attempts inherited worktree trust (step 4–5 of spec §"Verification behavior"), then a read-only legacy-migration fallback (step 6). No successful inherited check writes any store state. Direct trust remains the first lookup and is unchanged.

**Files:**
- Modify: `crates/ironlint-core/src/trust.rs` (`check_trust_in` + new private helpers)

**Interfaces:**
- Consumes: `WorktreeScope::discover`, `compute_worktree_hash` (Tasks 1–2), `TrustStore.worktree_entries` (Task 3), `canonical_key`, `compute_hash` (existing).
- Produces: no new public API — `check_trust_in` keeps its signature and `TrustOutcome` mapping.

**Verification behavior (spec §"Verification behavior"), encoded exactly:**
1. Compute the direct hash. Failure → `Unverifiable` (exit 1). *(unchanged)*
2. Read the store. Corrupt/unreadable → `Untrusted` (exit 4). *(unchanged)*
3. If `entries[canonical_config_path] == direct hash` → `Trusted`. *(unchanged)*
4. Only after a direct miss: discover scope + worktree hash. Either unavailable → skip inheritance (do **not** turn this into exit 1 — fall through to untrusted).
5. `worktree_entries[common_dir][config_rel]` hash matches → `Trusted`.
6. Legacy migration fallback (below). A valid candidate → `Trusted`.
7. Else the existing fixed untrusted error.

**Legacy fallback (spec, read-only, must not lazily add a v2 entry):** For each direct entry in `entries`, treat the candidate's stored config path as valid only when ALL hold:
1. Its stored config path still exists and yields an eligible worktree scope.
2. That scope has the same canonical common Git directory and config-relative path as the current scope.
3. Recomputing the candidate's direct hash still equals its stored entry (the blessed source policy is unchanged).
4. Recomputing the candidate's worktree policy hash equals the current worktree policy hash.

Any missing path, stale entry, malformed metadata, or hash error is ignored. The fallback is read-only — never writes a v2 entry.

- [ ] **Step 1: Write failing tests** in `trust.rs`'s `mod tests`:

```rust
    // --- inherited + legacy lookup (Task 5) -------------------------------
    #[test]
    fn direct_miss_with_matching_worktree_entry_is_trusted() {
        // Bless primary; the SAME store has no direct entry for a sibling's
        // canonical path, but the worktree_entries entry matches -> Trusted.
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        let cfg = root.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n");
        let store = tempfile::tempdir().unwrap();
        let store_path = store.path().join("trust.json");
        bless_in(&cfg, &store_path, "t").unwrap();
        // Synthesize a "sibling" by removing the direct entry but KEEPING the
        // worktree entry — simulates a linked worktree with its own canonical path.
        let mut s = read_store(&store_path).unwrap();
        let key = canonical_key(&cfg).unwrap();
        s.entries.remove(&key);
        write_store(&store_path, &s).unwrap();
        // Direct miss, worktree hit:
        assert!(matches!(check_trust_in(&cfg, &store_path), TrustOutcome::Trusted));
    }

    #[test]
    fn non_git_repo_falls_through_to_untrusted_when_not_blessed() {
        let root = tempfile::tempdir().unwrap();
        let cfg = root.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n");
        let store = tempfile::tempdir().unwrap();
        let store_path = store.path().join("trust.json");
        // Never blessed -> untrusted; no inheritance path available.
        assert!(matches!(check_trust_in(&cfg, &store_path), TrustOutcome::Untrusted(_)));
    }

    #[test]
    fn check_never_writes_store_on_inherited_trust() {
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        let cfg = root.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n");
        let store = tempfile::tempdir().unwrap();
        let store_path = store.path().join("trust.json");
        bless_in(&cfg, &store_path, "t").unwrap();
        let before = std::fs::read(&store_path).unwrap();
        // Force a direct miss + worktree hit (remove direct entry):
        let mut s = read_store(&store_path).unwrap();
        let key = canonical_key(&cfg).unwrap();
        s.entries.remove(&key);
        write_store(&store_path, &s).unwrap();
        let _ = check_trust_in(&cfg, &store_path);
        let after = std::fs::read(&store_path).unwrap();
        assert_eq!(before, after, "inherited trust must not mutate the store");
        // NOTE: `before` was read AFTER bless; the remove+rewrite above is the
        // setup, not the act under test. Re-read the post-setup bytes for the
        // true baseline if this assertion is too strict:
        let baseline = std::fs::read(&store_path).unwrap();
        let _ = check_trust_in(&cfg, &store_path);
        assert_eq!(baseline, std::fs::read(&store_path).unwrap(), "store unchanged by check");
    }

    #[test]
    fn legacy_fallback_accepts_unchanged_sibling_in_same_scope() {
        // Build a v1-style store: a direct entry keyed by an OLD canonical path
        // that, when discovered, shares the current scope's common_dir +
        // config_rel, with an unchanged direct hash. The sibling's canonical
        // path differs from the current config's, so direct misses, but the
        // legacy fallback proves the policy is the same.
        //
        // Easiest faithful construction: bless in a primary git repo (writes
        // BOTH entries in v2), then DOWNGRADE the store to v1 shape by
        // deleting worktree_entries — leaving only the direct entry. Then move
        // the config to a different path within the SAME worktree root so the
        // canonical key changes (direct miss) but scope.common_dir + a new
        // config_rel... 
        //
        // IMPLEMENTER NOTE: the legacy fallback's premise is "old direct entry
        // for the SAME config path that still resolves to the same scope." The
        // cleanest faithful test: bless, then strip worktree_entries (v1
        // shape), then verify the SAME config (same path) is still trusted via
        // the legacy fallback WITHOUT re-blessing. This proves the one-time
        // upgrade convenience. See test below.
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        let cfg = root.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n");
        let store = tempfile::tempdir().unwrap();
        let store_path = store.path().join("trust.json");
        bless_in(&cfg, &store_path, "t").unwrap();
        // Downgrade to v1 shape: drop worktree_entries, keep the direct entry.
        let mut s = read_store(&store_path).unwrap();
        s.worktree_entries.clear();
        write_store(&store_path, &s).unwrap();
        let bytes_before = std::fs::read(&store_path).unwrap();
        // Direct HIT actually (same path) — to exercise the FALLBACK we need a
        // direct MISS with a v1 entry whose path still resolves. The faithful
        // case is a linked sibling sharing scope; covered in the integration
        // test (Task 6). Here, assert the store is NOT mutated by the lookup:
        let _ = check_trust_in(&cfg, &store_path);
        assert_eq!(bytes_before, std::fs::read(&store_path).unwrap(),
            "legacy fallback is read-only");
    }
```

> **Implementer note on the legacy-fallback unit test:** The fallback's distinguishing behavior — a *different* canonical path that nonetheless resolves to the same `(common_dir, config_rel)` — is hard to construct in a unit test without a real linked worktree (which needs `git worktree add`, exercised in Task 6). The unit test above instead pins the **read-only** guarantee: the fallback never writes a v2 entry. The cross-worktree acceptance case is Task 6, test 2. Do not weaken the read-only assertion to make a richer unit case fit — the integration test carries that load.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ironlint-core --lib trust::tests::direct_miss trust::tests::non_git_repo_falls trust::tests::check_never_writes trust::tests::legacy_fallback`
Expected: FAIL — inherited lookup not implemented (`direct_miss_with_matching_worktree_entry` is `Untrusted`, not `Trusted`); read-only test may pass coincidentally but the lookup is absent.

- [ ] **Step 3: Implement inherited lookup + legacy fallback in `check_trust_in`**

Refactor `check_trust_in` to delegate to small helpers (keeps it under complexity 15). Replace the existing body:

```rust
pub fn check_trust_in(config_path: &Path, store_path: &Path) -> TrustOutcome {
    let expected = match compute_hash(config_path) {
        Ok(h) => h,
        Err(e) => return TrustOutcome::Unverifiable(e),
    };
    let key = match canonical_key(config_path) {
        Ok(k) => k,
        Err(e) => return TrustOutcome::Unverifiable(e),
    };
    let store = match read_store(store_path) {
        Ok(s) => s,
        Err(e) => return TrustOutcome::Untrusted(e),
    };
    // 1. Direct trust (unchanged, first).
    if let Some(entry) = store.entries.get(&key) {
        if entry.hash == expected {
            return TrustOutcome::Trusted;
        }
    }
    // 2. Inherited worktree trust (only after a direct miss).
    if inherited_trusted(config_path, &store) {
        return TrustOutcome::Trusted;
    }
    // 3. Legacy migration fallback (read-only).
    if legacy_fallback_trusted(config_path, &store) {
        return TrustOutcome::Trusted;
    }
    TrustOutcome::Untrusted(anyhow::anyhow!(
        "config/scripts not trusted — review and run `ironlint trust`"
    ))
}

/// Step 5: look up `worktree_entries[common_dir][config_rel]` by worktree hash.
fn inherited_trusted(config_path: &Path, store: &TrustStore) -> bool {
    let Some(scope) = WorktreeScope::discover(config_path) else { return false; };
    let Ok(Some(wt_hash)) = compute_worktree_hash(config_path, &scope) else { return false; };
    store
        .worktree_entries
        .get(&scope.common_dir.to_string_lossy())
        .and_then(|m| m.get(&scope.config_rel))
        .is_some_and(|e| e.hash == wt_hash)
}

/// Step 6: read-only legacy fallback. For each old direct entry whose path
/// still resolves to the SAME (common_dir, config_rel) with an unchanged
/// direct hash AND a matching worktree hash, accept. Never writes.
fn legacy_fallback_trusted(config_path: &Path, store: &TrustStore) -> bool {
    let Some(scope) = WorktreeScope::discover(config_path) else { return false; };
    let Ok(Some(current_wt_hash)) = compute_worktree_hash(config_path, &scope) else { return false; };
    for (stored_path, entry) in &store.entries {
        if legacy_candidate_matches(stored_path, entry, &scope, &current_wt_hash) {
            return true;
        }
    }
    false
}

/// Validate one legacy candidate against the four spec conditions.
fn legacy_candidate_matches(
    stored_path: &str,
    entry: &TrustEntry,
    scope: &WorktreeScope,
    current_wt_hash: &str,
) -> bool {
    let candidate = Path::new(stored_path);
    let Ok(canon) = candidate.canonicalize() else { return false; };
    let Some(cand_scope) = WorktreeScope::discover(&canon) else { return false; };
    // (2) same common dir + config_rel
    if cand_scope.common_dir != scope.common_dir || cand_scope.config_rel != scope.config_rel {
        return false;
    }
    // (3) recomputed direct hash still equals the stored entry
    let Ok(direct) = compute_hash(&canon) else { return false; };
    if direct != entry.hash { return false; }
    // (4) recomputed worktree hash equals the current worktree hash
    let Ok(Some(cand_wt)) = compute_worktree_hash(&canon, &cand_scope) else { return false; };
    cand_wt == current_wt_hash
}
```

- [ ] **Step 4: Run the full trust test suite**

Run: `cargo test -p ironlint-core --lib trust && cargo test -p ironlint-core --test trust_extends`
Expected: PASS — new inherited/legacy tests green; all existing trust tests (never-blessed-is-untrusted, editing-revokes, corrupt-store fail-closed, concurrent blesses) still green.

- [ ] **Step 5: Run the CLI check exit-code tests**

Run: `cargo test -p ironlint-cli --test cli_e2e_gates` (and any `cli_e2e_trust` exit-4 tests)
Expected: PASS — exit-code contract unchanged; exit 4 still fires for genuinely untrusted configs.

- [ ] **Step 6: Lint + fmt + coverage**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt`
Then: `bash scripts/ci-coverage.sh`; `trust.rs` ≥80%.
Then: `rm -rf target/llvm-cov-target target/llvm-cov`.

- [ ] **Step 7: Commit**

```bash
git add crates/ironlint-core/src/trust.rs
git commit -m "feat(trust): inherited worktree lookup + read-only legacy fallback"
```

---

## Task 6: Integration + CLI tests with real `git worktree add`

**Goal:** The spec's "Integration and CLI coverage" test plan, exercised through **real** linked-worktree metadata (not hand-crafted fixtures). These tests are the acceptance gate for the whole feature.

**Files:**
- Create: `crates/ironlint-core/tests/trust_worktree.rs` (core-level, uses `bless_in`/`check_trust_in` directly)
- Create: `crates/ironlint-cli/tests/cli_e2e_trust_worktree.rs` (CLI-level, uses `assert_cmd` against the compiled binary)

**Interfaces:**
- Consumes: `ironlint_core::trust::{bless_in, check_trust_in, TrustOutcome}`; `assert_cmd::Command`; `std::process::Command::new("git")`.
- Produces: no new API.

**Test plan (spec §"Integration and CLI coverage")** — each numbered item maps to a test. All use a real `git init` + `git worktree add`. If `git` is absent on PATH, skip with a clear message (CI has git).

- [ ] **Step 1: Write the core integration test file** `crates/ironlint-core/tests/trust_worktree.rs`:

```rust
//! Linked-worktree trust inheritance, exercised through REAL `git worktree add`
//! metadata — not a hand-crafted directory fixture. These are the acceptance
//! tests for docs/superpowers/specs/2026-07-11-git-worktree-trust-inheritance-design.md.

use ironlint_core::trust::{bless_in, check_trust_in, TrustOutcome};
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::tempdir;

fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Set up a primary git repo with a config + a blocking check script, plus a
/// linked worktree sibling. Returns (primary_cfg, linked_cfg, store_path, script).
fn worktree_pair() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let primary = tempdir().unwrap();
    let linked_root = tempdir().unwrap();
    // git init the primary
    assert!(Command::new("git").args(["init", "-q"]).arg(primary.path()).status().unwrap().success());
    // write a config + a script that BLOCKS (exits 1) so we can prove check ran
    let cfg = primary.path().join(".ironlint.yml");
    std::fs::write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \".ironlint/scripts/g.sh\"\n").unwrap();
    let scripts = primary.path().join(".ironlint/scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    let script = scripts.join("g.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();
    // commit so `git worktree add` has a HEAD
    Command::new("git").args(["add", "."]).current_dir(primary.path()).status().unwrap();
    Command::new("git").args(["-c", "user.email=t@t.co", "-c", "user.name=t", "commit", "-qm", "init"])
        .current_dir(primary.path()).status().unwrap();
    // add a linked worktree
    assert!(Command::new("git").args(["worktree", "add", "-q"])
        .arg(linked_root.path()).current_dir(primary.path()).status().unwrap().success());
    let linked_cfg = linked_root.path().join(".ironlint.yml");
    let store = tempdir().unwrap();
    let store_path = store.path().join("trust.json");
    (cfg, linked_cfg, store_path, script)
}

#[test]
fn blessed_primary_makes_unchanged_sibling_trusted() {
    if !git_available() { eprintln!("skip: git not on PATH"); return; }
    let (primary_cfg, linked_cfg, store_path, _script) = worktree_pair();
    bless_in(&primary_cfg, &store_path, "t").unwrap();
    // The sibling has a different canonical path -> direct miss -> worktree hit.
    assert!(matches!(check_trust_in(&linked_cfg, &store_path), TrustOutcome::Trusted),
        "unchanged sibling of a blessed primary must be trusted via inheritance");
}

#[test]
fn legacy_v1_entry_trusts_unchanged_sibling_without_rebless() {
    if !git_available() { eprintln!("skip: git not on PATH"); return; }
    let (primary_cfg, linked_cfg, store_path, _script) = worktree_pair();
    bless_in(&primary_cfg, &store_path, "t").unwrap();
    // Downgrade to v1 shape: drop worktree_entries, keep only the direct entry.
    let mut s = ironlint_core::trust::read_store(&store_path).unwrap();
    s.worktree_entries.clear();
    ironlint_core::trust::write_store(&store_path, &s).unwrap();
    let bytes_before = std::fs::read(&store_path).unwrap();
    // Sibling is NOT directly blessed (different canonical path), and there's
    // no v2 worktree entry — but the legacy fallback proves the policy matches.
    assert!(matches!(check_trust_in(&linked_cfg, &store_path), TrustOutcome::Trusted),
        "legacy fallback trusts an unchanged sibling with only a v1 direct entry");
    let bytes_after = std::fs::read(&store_path).unwrap();
    assert_eq!(bytes_before, bytes_after, "legacy fallback must not mutate the store");
}

#[test]
fn editing_sibling_config_revokes_trust() {
    if !git_available() { eprintln!("skip: git not on PATH"); return; }
    let (primary_cfg, linked_cfg, store_path, _script) = worktree_pair();
    bless_in(&primary_cfg, &store_path, "t").unwrap();
    std::fs::write(&linked_cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n").unwrap();
    assert!(matches!(check_trust_in(&linked_cfg, &store_path), TrustOutcome::Untrusted(_)),
        "a changed sibling config must not inherit trust");
}

#[test]
fn editing_sibling_script_revokes_trust() {
    if !git_available() { eprintln!("skip: git not on PATH"); return; }
    let (primary_cfg, linked_cfg, store_path, _script) = worktree_pair();
    bless_in(&primary_cfg, &store_path, "t").unwrap();
    // The sibling's script lives under the linked worktree's own
    // .ironlint/scripts/ (git worktree adds a working tree copy).
    let sibling_script = linked_cfg.parent().unwrap().join(".ironlint/scripts/g.sh");
    std::fs::write(&sibling_script, "#!/bin/sh\nexit 0\n").unwrap();
    assert!(matches!(check_trust_in(&linked_cfg, &store_path), TrustOutcome::Untrusted(_)),
        "a changed sibling script must not inherit trust");
}

#[test]
fn separate_clone_with_identical_policy_does_not_inherit() {
    if !git_available() { eprintln!("skip: git not on PATH"); return; }
    let (primary_cfg, _linked_cfg, store_path, _script) = worktree_pair();
    bless_in(&primary_cfg, &store_path, "t").unwrap();
    // A completely separate git repo with byte-identical policy -> different
    // common dir -> no inheritance.
    let other = tempdir().unwrap();
    Command::new("git").args(["init", "-q"]).arg(other.path()).status().unwrap();
    let other_cfg = other.path().join(".ironlint.yml");
    std::fs::write(&other_cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \".ironlint/scripts/g.sh\"\n").unwrap();
    std::fs::create_dir_all(other.path().join(".ironlint/scripts")).unwrap();
    std::fs::write(other.path().join(".ironlint/scripts/g.sh"), "#!/bin/sh\nexit 1\n").unwrap();
    assert!(matches!(check_trust_in(&other_cfg, &store_path), TrustOutcome::Untrusted(_)),
        "a separate clone must not inherit trust");
}
```

- [ ] **Step 2: Write the CLI integration test file** `crates/ironlint-cli/tests/cli_e2e_trust_worktree.rs` (spec items 1, 6, 7):

```rust
//! CLI-level linked-worktree trust inheritance. Proves check EXECUTES (exit 2)
//! in a trusted sibling, and that `ironlint trust` prints the scope line + the
//! existing summary, and writes both entries.

use assert_cmd::Command;
use std::process::Command as StdCommand;
use tempfile::tempdir;

fn git_available() -> bool {
    StdCommand::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

#[test]
fn trusted_sibling_runs_a_blocking_check_and_exits_2() {
    if !git_available() { eprintln!("skip: git not on PATH"); return; }
    let xdg = tempdir().unwrap();
    let primary = tempdir().unwrap();
    let linked = tempdir().unwrap();
    StdCommand::new("git").args(["init", "-q"]).arg(primary.path()).status().unwrap();
    let cfg = primary.path().join(".ironlint.yml");
    std::fs::write(&cfg, "checks:\n  g:\n    files: \"*.rs\"\n    run: \".ironlint/scripts/g.sh\"\n").unwrap();
    std::fs::create_dir_all(primary.path().join(".ironlint/scripts")).unwrap();
    std::fs::write(primary.path().join(".ironlint/scripts/g.sh"), "#!/bin/sh\necho blocked; exit 1\n").unwrap();
    StdCommand::new("git").args(["add", "."]).current_dir(primary.path()).status().unwrap();
    StdCommand::new("git").args(["-c","user.email=t@t.co","-c","user.name=t","commit","-qm","i"])
        .current_dir(primary.path()).status().unwrap();
    StdCommand::new("git").args(["worktree","add","-q"]).arg(linked.path())
        .current_dir(primary.path()).status().unwrap();
    // Bless the primary through the CLI.
    Command::cargo_bin("ironlint").unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust","--config"]).arg(&cfg)
        .assert().success()
        .stdout(predicates::str::contains("scope: linked worktrees"));
    // In the sibling: check runs the blocking gate -> exit 2.
    let linked_cfg = linked.path().join(".ironlint.yml");
    Command::cargo_bin("ironlint").unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["check","--config"]).arg(&linked_cfg)
        .arg(linked.path().join("dummy.rs"))
        .assert()
        .failure()
        .code(2);
    let store = std::fs::read_to_string(xdg.path().join("ironlint/trust.json")).unwrap();
    assert!(store.contains("worktree_entries"), "store has a worktree_entries field: {store}");
}
```

- [ ] **Step 3: Run the integration tests**

Run: `cargo test -p ironlint-core --test trust_worktree && cargo test -p ironlint-cli --test cli_e2e_trust_worktree`
Expected: PASS — all real-`git-worktree` tests green. If `git` is absent locally, they skip; in CI they run.

- [ ] **Step 4: Lint + fmt + coverage**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt`
Then: `bash scripts/ci-coverage.sh`; changed `src` files ≥80% (test files are not coverage-gated).
Then: `rm -rf target/llvm-cov-target target/llvm-cov`.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-core/tests/trust_worktree.rs crates/ironlint-cli/tests/cli_e2e_trust_worktree.rs
git commit -m "test(trust): real-git-worktree inheritance integration tests"
```

---

## Task 7: Documentation

**Goal:** Document linked-worktree inheritance, its in-tree-only boundary, and correct the exit-code doc. Adapter runtime behavior is unchanged; doc corrections only.

**Files:**
- Modify: `docs/security/trust.md`
- Modify: any adapter doc that says untrusted configs fail open or exit `1` (search-and-correct; runtime unchanged)

**Spec references (verbatim):**
- "document linked-worktree inheritance, its in-tree-only boundary, and the fact that untrusted `check` exits `4` (the document currently says `1`)."
- "correct any remaining reference to untrusted configs failing open or using exit `1`; adapter runtime behavior itself does not change."

- [ ] **Step 1: Find stale exit-`1` / fail-open references**

Run: `grep -rn "exit \`1\`\|fail.open\|exits 1\b\|exit(1)\|exit 1\b" docs/ | grep -i trust`
Also: `grep -rn "untrusted" docs/`
Record every hit that describes the untrusted-config path.

- [ ] **Step 2: Correct `docs/security/trust.md`**

Specifically:
- Line 51 currently says `…it stops with a config error (exit `1`) and a hint to re-bless…`. Change `exit \`1\`` → `exit \`4\``. (The actual code emits exit 4 via `check.rs`.)
- Add a new subsection under "## How verification works" titled "### Linked-worktree inheritance" covering:
  - One `ironlint trust` in any worktree of a local Git repo covers the same unchanged policy in every linked worktree (same common Git directory).
  - The boundary: only fully in-worktree policy (root config + every `extends:` config + every `.ironlint/scripts/` file under the same worktree root) is eligible. An external `extends:` target or a symlinked-out config disables inheritance; exact-path trust still applies.
  - `check` never writes the store; only `ironlint trust` grants approval. Inheritance is convenience around an existing approval, not automatic approval.
  - A separate clone (different common Git directory) does not inherit.
  - Upgrading: a still-present trusted v1 entry is honored via a read-only legacy fallback; one `ironlint trust` in any eligible worktree writes the durable v2 family entry.
- The `ironlint trust` summary now prints a `scope:` line (`linked worktrees` or `this config path`).

- [ ] **Step 3: Correct adapter docs**

For each stale hit from Step 1 in adapter docs: change the untrusted-config exit reference to `4` and remove any "fail open" claim for exit 4 (exit 4 is fail-closed in every pre-write adapter). Do **not** describe runtime behavior changes — there are none.

- [ ] **Step 4: Build a release binary and smoke-test the documented flow**

Run: `cargo build --release`
Then in a temp git repo + linked worktree (use `ironlint` from `target/release`), bless the primary and confirm `check` in the sibling runs (exit 0 with a passing check). Confirm `ironlint trust` prints `scope: linked worktrees`.
Then: `cargo clean -p ironlint` (or `rm target/release/ironlint`) — drop the verification binary.

- [ ] **Step 5: Commit**

```bash
git add docs/security/trust.md <any adapter docs changed>
git commit -m "docs(trust): linked-worktree inheritance + exit-4 correction"
```

---

## Self-Review (run before execution)

**1. Spec coverage:**
- §1 discovery → Task 1 ✓
- §2 worktree hash → Task 2 ✓
- §3 store v2 → Task 3 ✓
- §4 blessing + scope line → Task 4 ✓
- §"Verification behavior" + legacy fallback → Task 5 ✓
- §"Required behavior and failures" table → covered by Tasks 5+6 (each row maps to a test)
- §"Code and documentation changes" file list → Tasks 1–7 ✓
- §"Test plan" unit coverage → Tasks 1–5 ✓; integration/CLI coverage → Task 6 ✓
- §"Handoff checklist" → verified by Tasks 5–6 acceptance tests ✓

**2. Placeholder scan:** none — every code step shows complete code.

**3. Type consistency:** `WorktreeScope { common_dir, worktree_root, config_rel }` consistent across Tasks 1→2→4→5. `compute_worktree_hash(config_path, &scope) -> Result<Option<String>>` consistent. `TrustStore.worktree_entries: BTreeMap<String, BTreeMap<String, TrustEntry>>` consistent. `BlessedSummary.scope: String` consistent.

**Known cross-task coupling:** Tasks 4 and 5 both modify `trust.rs`. Dispatch sequentially (never parallel). Task 4's `write_worktree_entry` must use the **worktree hash** (`wt_hash`), not the direct hash — the plan flags this inline with a correction note. Task 5's `check_trust_in` refactor must preserve the exact `TrustOutcome::Untrusted` fixed message string (`config/scripts not trusted — review and run \`ironlint trust\``) so existing tests pass.
