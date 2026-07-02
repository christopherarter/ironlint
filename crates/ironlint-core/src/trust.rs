use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Feed one labeled blob into the hasher with length prefixes on both the
/// label and the content, so no two distinct (label, bytes) pairs can collide
/// by concatenation.
fn hash_entry(hasher: &mut Sha256, label: &str, bytes: &[u8]) {
    hasher.update((label.len() as u64).to_le_bytes());
    hasher.update(label.as_bytes());
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Filesystem classification of a path in the gates hash walk, computed via
/// `symlink_metadata` (which does **not** follow symlinks) rather than
/// `is_dir()`/`is_file()` (which do).
#[derive(Debug)]
enum EntryKind {
    Dir,
    File,
    Missing,
}

/// Classify `path` for the gates hash walk without ever following a
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
/// means for its position in the walk (e.g. an absent gates dir has
/// nothing to hash). A path whose parent isn't even a directory (e.g. a
/// plain file sits where `.ironlint/` should be) is treated the same as
/// missing: there is nothing there to hash, and this isn't the
/// symlink/non-regular-file class of problem the walk is guarding against.
fn classify_entry(path: &Path) -> Result<EntryKind> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            let file_type = meta.file_type();
            if file_type.is_symlink() {
                anyhow::bail!(
                    "gates dir contains a symlink ({}); refuse to hash — replace it with a regular file",
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
                "gates dir contains a non-regular file ({}); refuse to hash",
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
fn collect_gate_files(dir: &Path) -> Result<Vec<(String, Vec<u8>)>> {
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
                    .expect("walked path must live under the gates root")
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
                // loudly rather than silently under-hashing the gates dir.
                anyhow::bail!("gates dir entry disappeared mid-walk ({})", path.display());
            }
        }
    }
    Ok(())
}

/// Compute the trust hash of a config over its **entire `extends:` closure**.
///
/// Sha256 over every config file reachable from `config_path` (the root plus
/// every transitively-extended file) plus every file under each participating
/// config dir's `.ironlint/gates/`. Returns `"sha256:<hex>"`.
///
/// Folding only the root config would let a blessed child that `extends:` a base
/// have that base — or the base's gate scripts — swapped under it without
/// invalidating the hash. Every blob is folded with [`hash_entry`]'s
/// length-prefixed framing and a label bound to the blob's identity (its
/// canonical config path, or its gates dir + relative path), so neither
/// reordering nor relabeling can produce a collision. A no-`extends:` config
/// resolves to a one-element closure and keeps its prior behaviour: its own
/// edits, and edits to its own gate scripts, still revoke trust.
pub fn compute_hash(config_path: &Path) -> Result<String> {
    let mut hasher = Sha256::new();

    let config_paths = crate::config::extends::resolve_paths(config_path)
        .with_context(|| format!("resolving extends closure for {}", config_path.display()))?;

    // Fold each config file, keyed by its canonical path.
    for path in &config_paths {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        hash_entry(&mut hasher, &format!("config\0{}", path.display()), &bytes);
    }

    // Fold gate scripts under each distinct participating config dir's
    // `.ironlint/gates/`. Dedup so a shared dir is never double-folded.
    let mut gate_dirs: Vec<PathBuf> = config_paths
        .iter()
        .map(|p| {
            p.parent()
                .unwrap_or_else(|| Path::new("."))
                .join(".ironlint")
                .join("gates")
        })
        .collect();
    gate_dirs.sort();
    gate_dirs.dedup();

    for gates_dir in &gate_dirs {
        match classify_entry(gates_dir)? {
            EntryKind::Dir => {
                for (rel, bytes) in collect_gate_files(gates_dir)? {
                    hash_entry(
                        &mut hasher,
                        &format!("gates\0{}\0{rel}", gates_dir.display()),
                        &bytes,
                    );
                }
            }
            EntryKind::Missing => {}
            EntryKind::File => {
                anyhow::bail!(
                    "expected {} to be a directory (gates dir)",
                    gates_dir.display()
                );
            }
        }
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

pub const TRUST_STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustStore {
    /// Schema version. A defaulted (never-written) store has `0`; a store
    /// written by `bless` carries `TRUST_STORE_VERSION`. Trust decisions key
    /// off per-entry hashes, not this field — it exists for future migrations.
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub entries: BTreeMap<String, TrustEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEntry {
    pub hash: String,
    pub blessed_at: String,
}

/// `$XDG_CONFIG_HOME` (if set and non-empty) else `$HOME/.config`. Pure
/// resolver split out from the env read so it is testable without mutating
/// process env.
fn config_home_from(xdg: Option<String>, home: Option<String>) -> Option<PathBuf> {
    if let Some(x) = xdg {
        if !x.is_empty() {
            return Some(PathBuf::from(x));
        }
    }
    home.map(|h| PathBuf::from(h).join(".config"))
}

pub fn config_home() -> Option<PathBuf> {
    config_home_from(
        std::env::var("XDG_CONFIG_HOME").ok(),
        std::env::var("HOME").ok(),
    )
}

fn store_path_in(config_home: &Path) -> PathBuf {
    config_home.join("ironlint").join("trust.json")
}

/// Absolute path to the out-of-repo trust store.
pub fn trust_store_path() -> Result<PathBuf> {
    let home = config_home().ok_or_else(|| {
        anyhow::anyhow!("cannot resolve config home (set $XDG_CONFIG_HOME or $HOME)")
    })?;
    Ok(store_path_in(&home))
}

/// Read the store; a missing file yields an empty store (never an error).
pub fn read_store(path: &Path) -> Result<TrustStore> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TrustStore::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// A sibling temp path unique to this call: `json.tmp.<pid>.<counter>`. Two
/// writers targeting the same `path` (different processes, or different
/// threads within one process racing the same store) never share a temp
/// file, so one writer's write+rename can never clobber or be clobbered by
/// another's mid-flight. The counter alone (no clock read) is enough:
/// per-process it's strictly increasing, and across processes a stale
/// leftover temp file at a reused pid+counter pair is simply overwritten
/// before anyone reads it, since only the final `rename` target is load
/// bearing.
fn unique_tmp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!("json.tmp.{}.{n}", std::process::id()))
}

/// Write the store atomically: serialize to a sibling, per-write-unique temp
/// file, then rename over the target.
pub fn write_store(path: &Path, store: &TrustStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(store)?;
    let tmp = unique_tmp_path(path);
    std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

/// Sibling lock-file path used to serialize concurrent `bless_in`
/// read-modify-writes against the same store.
fn lock_path(store_path: &Path) -> PathBuf {
    store_path.with_extension("lock")
}

/// RAII guard for the exclusive store lock: dropping it releases the flock
/// (closing the underlying fd releases a POSIX `flock`, so an explicit
/// `unlock` call isn't needed here — unlike `telemetry::append`, which
/// unlocks eagerly to shrink its critical section).
#[cfg(unix)]
struct StoreLock {
    _file: std::fs::File,
}

#[cfg(unix)]
fn acquire_store_lock(store_path: &Path) -> Result<StoreLock> {
    use fs4::fs_std::FileExt;
    if let Some(parent) = store_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(store_path))
        .with_context(|| format!("opening lock file for {}", store_path.display()))?;
    FileExt::lock_exclusive(&file).with_context(|| format!("locking {}", store_path.display()))?;
    Ok(StoreLock { _file: file })
}

/// No file-locking primitive is wired up for non-unix targets yet (tracked
/// alongside the broader Windows-support gap); blessing still works, just
/// without cross-process serialization.
#[cfg(not(unix))]
struct StoreLock;

#[cfg(not(unix))]
fn acquire_store_lock(store_path: &Path) -> Result<StoreLock> {
    if let Some(parent) = store_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    Ok(StoreLock)
}

/// Classify the outcome of reading the raw store bytes for a bless. Pure
/// (independent of the filesystem) so the classification can be unit tested
/// without permission-flakiness.
///
/// Only a **parse** failure on content that was actually read is tolerated
/// as "empty store" (with a warning to stderr), so a corrupt or
/// half-written store can self-heal the next time someone runs
/// `ironlint trust`. A missing file is also empty (first bless). Any other
/// read failure (permission denied, transient I/O, ...) propagates as
/// `Err` rather than being treated as empty — swallowing it would let
/// `bless_in` silently overwrite a real, non-empty store with just the one
/// new entry, since `rename` only needs directory write permission, not
/// read permission on the target file. [`ensure_trusted_in`] deliberately
/// does **not** use this path — a corrupt store must keep `check` failing
/// closed.
fn classify_store_read(store_path: &Path, read: std::io::Result<String>) -> Result<TrustStore> {
    match read {
        Ok(s) => Ok(serde_json::from_str(&s).unwrap_or_else(|_| {
            eprintln!(
                "warning: trust store at {} was unreadable; rewriting",
                store_path.display()
            );
            TrustStore::default()
        })),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TrustStore::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", store_path.display())),
    }
}

/// Read the store for a bless. See [`classify_store_read`] for the
/// tolerance rules.
fn read_store_for_bless(store_path: &Path) -> Result<TrustStore> {
    classify_store_read(store_path, std::fs::read_to_string(store_path))
}

/// Canonical absolute path used as the store key for `config_path`.
fn canonical_key(config_path: &Path) -> Result<String> {
    let canon = config_path
        .canonicalize()
        .with_context(|| format!("resolving {}", config_path.display()))?;
    Ok(canon.to_string_lossy().to_string())
}

/// Verify `config_path` (and its gate scripts) match a blessed entry in the
/// store at `store_path`. Fails closed with a fixed, actionable message.
pub fn ensure_trusted_in(config_path: &Path, store_path: &Path) -> Result<()> {
    let key = canonical_key(config_path)?;
    let expected = compute_hash(config_path)?;
    let store = read_store(store_path)?;
    match store.entries.get(&key) {
        Some(entry) if entry.hash == expected => Ok(()),
        _ => anyhow::bail!("config/gates not trusted — review and run `ironlint trust`"),
    }
}

/// Recompute the hash of `config_path` and write it to the store as blessed.
///
/// Validates the full `extends:` closure first (not just the local file) so a
/// config whose `extends:` target is missing or broken is never blessed. The
/// read-modify-write against `store_path` is guarded by an exclusive
/// [`acquire_store_lock`], so concurrent `ironlint trust` runs (e.g. parallel
/// agent sessions) serialize instead of racing and losing entries; a corrupt
/// existing store is tolerated (see [`read_store_for_bless`]) so blessing
/// doubles as the recovery path.
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
        TrustEntry {
            hash,
            blessed_at: now.to_string(),
        },
    );
    write_store(store_path, &store)
}

/// Thin wrapper: enforce trust against the real out-of-repo store.
pub fn ensure_trusted(config_path: &Path) -> Result<()> {
    ensure_trusted_in(config_path, &trust_store_path()?)
}

/// Thin wrapper: bless against the real out-of-repo store, stamping `blessed_at`
/// with the current UTC time.
pub fn bless(config_path: &Path) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    bless_in(config_path, &trust_store_path()?, &now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn hash_is_deterministic_and_prefixed() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
        );
        let a = compute_hash(&cfg).unwrap();
        let b = compute_hash(&cfg).unwrap();
        assert_eq!(a, b, "same inputs must hash identically");
        assert!(
            a.starts_with("sha256:"),
            "hash must be sha256-prefixed: {a}"
        );
    }

    #[test]
    fn editing_config_changes_hash() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
        );
        let before = compute_hash(&cfg).unwrap();
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \"false\"\n",
        );
        let after = compute_hash(&cfg).unwrap();
        assert_ne!(before, after, "a config edit must invalidate the hash");
    }

    #[test]
    fn editing_a_gate_script_changes_hash() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \".ironlint/gates/g.sh\"\n",
        );
        let script = dir.path().join(".ironlint/gates/g.sh");
        write(&script, "#!/bin/sh\nexit 0\n");
        let before = compute_hash(&cfg).unwrap();
        write(&script, "#!/bin/sh\nexit 2\n");
        let after = compute_hash(&cfg).unwrap();
        assert_ne!(before, after, "a gate-script edit must invalidate the hash");
    }

    #[test]
    fn hash_folds_gate_files_in_sorted_order() {
        // compute_hash must fold the config plus its gate files in a
        // deterministic, identity-bound frame: each config keyed by its
        // canonical path, each gate file keyed by its gates dir + sorted
        // relative path (independent of OS enumeration order). We pin the exact
        // scheme by recomputing the digest the same way using the impl's own
        // framing helper. This fails if the impl stops sorting gate files (the
        // `out.sort_by` in collect_gate_files) — on a filesystem whose read_dir
        // yields b before a — or if the label binding / length prefixes change,
        // which doubles as a regression lock on the stored-hash encoding.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        let cfg_body = "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n";
        write(&cfg, cfg_body);
        write(&dir.path().join(".ironlint/gates/a.sh"), "a\n");
        write(&dir.path().join(".ironlint/gates/b.sh"), "b\n");

        // Labels carry canonical paths, so recompute against the canonical form.
        let canon = cfg.canonicalize().unwrap();
        let gates_dir = canon.parent().unwrap().join(".ironlint").join("gates");

        let mut expected = Sha256::new();
        hash_entry(
            &mut expected,
            &format!("config\0{}", canon.display()),
            cfg_body.as_bytes(),
        );
        hash_entry(
            &mut expected,
            &format!("gates\0{}\0a.sh", gates_dir.display()),
            b"a\n",
        );
        hash_entry(
            &mut expected,
            &format!("gates\0{}\0b.sh", gates_dir.display()),
            b"b\n",
        );
        let want = format!("sha256:{:x}", expected.finalize());

        assert_eq!(compute_hash(&cfg).unwrap(), want);
    }

    #[test]
    fn compute_hash_errors_on_missing_extends_target() {
        // compute_hash resolves the extends closure; a config pointing at a
        // non-existent base can't be hashed — it fails closed rather than
        // silently hashing only the local file.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(&cfg, "extends: [\"./nope.yml\"]\nchecks: {}\n");
        let err = compute_hash(&cfg).unwrap_err().to_string();
        assert!(
            err.contains("extends closure"),
            "error should name the closure resolution: {err}"
        );
    }

    #[test]
    fn missing_gates_dir_hashes_only_the_config() {
        // No .ironlint/gates/ at all — must succeed (not error), hashing config alone.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
        );
        assert!(compute_hash(&cfg).unwrap().starts_with("sha256:"));
    }

    #[cfg(unix)]
    #[test]
    fn gates_dir_symlink_loop_is_a_clear_error_not_a_hang() {
        // A self-referencing symlink inside the gates dir must not be
        // followed. Before the fix, `is_dir()` follows the symlink and the
        // walk recurses through it; the OS eventually caps total symlink
        // resolutions and returns a raw ELOOP, so this terminates promptly
        // either way — but the *message* must call out the symlink, not
        // leak a raw OS error, and (after the fix) the walk must refuse the
        // symlink on first sight rather than ever recursing into it.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n");
        let gates = dir.path().join(".ironlint/gates");
        fs::create_dir_all(&gates).unwrap();
        write(&gates.join("g.sh"), "#!/bin/sh\nexit 0\n");
        std::os::unix::fs::symlink(&gates, gates.join("loop")).unwrap();

        let err = compute_hash(&cfg).unwrap_err().to_string();
        assert!(
            err.contains("symlink"),
            "error should call out the symlink, not a raw OS ELOOP: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn classify_entry_refuses_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        fs::write(&target, "hi").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = classify_entry(&link).unwrap_err().to_string();
        assert!(err.contains("symlink"), "error: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn classify_entry_refuses_a_fifo() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo must be available for this test");
        assert!(status.success(), "mkfifo failed");
        let err = classify_entry(&fifo).unwrap_err().to_string();
        assert!(err.contains("non-regular"), "error: {err}");
    }

    #[test]
    fn classify_entry_missing_path_is_missing_kind() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert!(matches!(
            classify_entry(&missing).unwrap(),
            EntryKind::Missing
        ));
    }

    #[test]
    fn classify_entry_dir_and_file_kinds() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, "x").unwrap();
        assert!(matches!(classify_entry(&sub).unwrap(), EntryKind::Dir));
        assert!(matches!(classify_entry(&file).unwrap(), EntryKind::File));
    }

    #[test]
    fn gate_files_recurse_into_subdirectories() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n");
        write(&dir.path().join(".ironlint/gates/top.sh"), "top\n");
        write(
            &dir.path().join(".ironlint/gates/sub/nested.sh"),
            "nested\n",
        );
        let before = compute_hash(&cfg).unwrap();
        write(
            &dir.path().join(".ironlint/gates/sub/nested.sh"),
            "nested changed\n",
        );
        let after = compute_hash(&cfg).unwrap();
        assert_ne!(
            before, after,
            "editing a nested gate file must change the hash"
        );
    }

    #[cfg(unix)]
    #[test]
    fn gates_dir_itself_as_symlink_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n");
        let real_gates = dir.path().join("real_gates");
        fs::create_dir_all(&real_gates).unwrap();
        write(&real_gates.join("g.sh"), "#!/bin/sh\nexit 0\n");
        fs::create_dir_all(dir.path().join(".ironlint")).unwrap();
        std::os::unix::fs::symlink(&real_gates, dir.path().join(".ironlint/gates")).unwrap();

        let err = compute_hash(&cfg).unwrap_err().to_string();
        assert!(err.contains("symlink"), "error: {err}");
    }

    #[test]
    fn gates_dir_path_is_a_plain_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n");
        fs::create_dir_all(dir.path().join(".ironlint")).unwrap();
        fs::write(dir.path().join(".ironlint/gates"), "not a dir").unwrap();
        assert!(compute_hash(&cfg).is_err());
    }

    #[test]
    fn store_path_joins_under_config_home() {
        let p = store_path_in(Path::new("/home/u/.config"));
        assert_eq!(p, Path::new("/home/u/.config/ironlint/trust.json"));
    }

    #[test]
    fn read_missing_store_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = read_store(&dir.path().join("trust.json")).unwrap();
        assert!(store.entries.is_empty());
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/trust.json"); // parent must be created
        let mut store = TrustStore {
            version: TRUST_STORE_VERSION,
            entries: std::collections::BTreeMap::new(),
        };
        store.entries.insert(
            "/abs/.ironlint.yml".to_string(),
            TrustEntry {
                hash: "sha256:abc".into(),
                blessed_at: "2026-06-24T00:00:00Z".into(),
            },
        );
        write_store(&path, &store).unwrap();
        let back = read_store(&path).unwrap();
        assert_eq!(back.entries["/abs/.ironlint.yml"].hash, "sha256:abc");
        assert_eq!(
            back.entries["/abs/.ironlint.yml"].blessed_at,
            "2026-06-24T00:00:00Z"
        );
        assert_eq!(back.version, TRUST_STORE_VERSION);
    }

    #[test]
    fn xdg_config_home_overrides_home() {
        // config_home() prefers XDG_CONFIG_HOME. Test the pure resolver with an
        // explicit value rather than mutating process env.
        assert_eq!(
            config_home_from(Some("/x".into()), Some("/h".into())),
            Some(PathBuf::from("/x"))
        );
        assert_eq!(
            config_home_from(None, Some("/h".into())),
            Some(PathBuf::from("/h/.config"))
        );
        // An empty XDG_CONFIG_HOME is treated as unset and falls through to HOME.
        assert_eq!(
            config_home_from(Some(String::new()), Some("/h".into())),
            Some(PathBuf::from("/h/.config"))
        );
        assert_eq!(config_home_from(None, None), None);
    }

    #[test]
    fn read_store_surfaces_non_notfound_errors() {
        // A path that exists but is a directory makes read_to_string fail with a
        // kind other than NotFound — that must propagate as Err, not be swallowed
        // into an empty store.
        let dir = tempfile::tempdir().unwrap();
        assert!(read_store(dir.path()).is_err());
    }

    fn cfg_with_gate(dir: &Path) -> PathBuf {
        let cfg = dir.join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*\"\n    run: \".ironlint/gates/g.sh\"\n",
        );
        write(&dir.join(".ironlint/gates/g.sh"), "#!/bin/sh\nexit 0\n");
        cfg
    }

    #[test]
    fn bless_then_ensure_succeeds() {
        let proj = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let store_path = store.path().join("trust.json");
        let cfg = cfg_with_gate(proj.path());
        bless_in(&cfg, &store_path, "2026-06-24T00:00:00Z").unwrap();
        assert!(ensure_trusted_in(&cfg, &store_path).is_ok());
    }

    #[test]
    fn never_blessed_is_not_trusted() {
        let proj = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let cfg = cfg_with_gate(proj.path());
        let err = ensure_trusted_in(&cfg, &store.path().join("trust.json"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("not trusted"),
            "message must say not trusted: {err}"
        );
        assert!(
            err.contains("ironlint trust"),
            "message must point at `ironlint trust`: {err}"
        );
    }

    #[test]
    fn editing_a_gate_after_bless_revokes_trust() {
        let proj = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let store_path = store.path().join("trust.json");
        let cfg = cfg_with_gate(proj.path());
        bless_in(&cfg, &store_path, "t").unwrap();
        // Tamper with the gate script.
        write(
            &proj.path().join(".ironlint/gates/g.sh"),
            "#!/bin/sh\nexit 2\n",
        );
        assert!(ensure_trusted_in(&cfg, &store_path).is_err());
    }

    #[test]
    fn editing_config_after_bless_revokes_trust() {
        let proj = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let store_path = store.path().join("trust.json");
        let cfg = cfg_with_gate(proj.path());
        bless_in(&cfg, &store_path, "t").unwrap();
        write(&cfg, "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n");
        assert!(ensure_trusted_in(&cfg, &store_path).is_err());
    }

    #[test]
    fn bless_rejects_unparseable_config() {
        let proj = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let cfg = proj.path().join(".ironlint.yml");
        write(&cfg, "schema_version: 2\nrules: {}\n"); // legacy → parser rejects
        assert!(bless_in(&cfg, &store.path().join("trust.json"), "t").is_err());
    }

    #[test]
    fn concurrent_blesses_do_not_lose_entries() {
        // Today, bless_in is an unlocked read-modify-write with a fixed temp
        // filename: N threads blessing distinct configs into the same store
        // race each other and lose entries. Line all N up on a barrier so
        // they hit the RMW at (as close to) the same instant as possible,
        // then assert every entry survived.
        const N: usize = 8;
        let store_dir = tempfile::tempdir().unwrap();
        let store_path = store_dir.path().join("trust.json");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(N));

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let store_path = store_path.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let proj = tempfile::tempdir().unwrap();
                    let cfg = cfg_with_gate(proj.path());
                    barrier.wait();
                    bless_in(&cfg, &store_path, "t").unwrap();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let store = read_store(&store_path).unwrap();
        assert_eq!(
            store.entries.len(),
            N,
            "concurrent blesses must not lose entries"
        );
    }

    #[test]
    fn bless_recovers_from_corrupt_store() {
        // A corrupt/half-written store must not brick `trust` — bless_in
        // should treat unparseable existing content as empty, warn, and
        // rewrite with the new entry rather than erroring out.
        let proj = tempfile::tempdir().unwrap();
        let store_dir = tempfile::tempdir().unwrap();
        let store_path = store_dir.path().join("trust.json");
        write(&store_path, "{ not json");
        let cfg = cfg_with_gate(proj.path());

        bless_in(&cfg, &store_path, "t").unwrap();

        let store = read_store(&store_path).unwrap();
        let key = canonical_key(&cfg).unwrap();
        assert!(
            store.entries.contains_key(&key),
            "bless must recover from a corrupt store and record the new entry"
        );
    }

    #[test]
    fn ensure_trusted_fails_closed_on_corrupt_store() {
        // Unlike bless_in, ensure_trusted must NOT tolerate corruption — a
        // corrupt store must keep `check` failing closed.
        let proj = tempfile::tempdir().unwrap();
        let store_dir = tempfile::tempdir().unwrap();
        let store_path = store_dir.path().join("trust.json");
        write(&store_path, "{ not json");
        let cfg = cfg_with_gate(proj.path());

        assert!(ensure_trusted_in(&cfg, &store_path).is_err());
    }

    #[test]
    fn unique_tmp_path_differs_across_calls() {
        let base = Path::new("/x/trust.json");
        let a = unique_tmp_path(base);
        let b = unique_tmp_path(base);
        assert_ne!(a, b, "temp names must be unique per write");
    }

    #[cfg(unix)]
    #[test]
    fn bless_in_fails_closed_when_store_unreadable_for_non_parse_reasons() {
        // read_store_for_bless must only tolerate a PARSE error (corrupt
        // content) as "empty store". A read that fails for any other reason
        // (permission denied, transient I/O) must propagate as Err rather
        // than being treated as an empty store — otherwise bless_in would
        // silently overwrite a real, non-empty store with just the one new
        // entry (rename only needs directory write permission, not read
        // permission on the target file, so the clobber would succeed).
        use std::os::unix::fs::PermissionsExt;

        let store_dir = tempfile::tempdir().unwrap();
        let store_path = store_dir.path().join("trust.json");

        // Seed a real, non-empty store first.
        let seed_proj = tempfile::tempdir().unwrap();
        let seed_cfg = cfg_with_gate(seed_proj.path());
        bless_in(&seed_cfg, &store_path, "t").unwrap();
        let before = fs::read(&store_path).unwrap();
        assert!(!before.is_empty());

        // Make the store file unreadable.
        let mut perms = fs::metadata(&store_path).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&store_path, perms).unwrap();

        // Root (and some sandboxed/CI environments) bypass file-mode
        // permission checks entirely — skip rather than pass vacuously.
        if std::fs::read_to_string(&store_path).is_ok() {
            let mut perms = fs::metadata(&store_path).unwrap().permissions();
            perms.set_mode(0o644);
            fs::set_permissions(&store_path, perms).unwrap();
            eprintln!(
                "skipping bless_in_fails_closed_when_store_unreadable_for_non_parse_reasons: \
                 running with privileges that bypass file-mode permissions"
            );
            return;
        }

        let proj = tempfile::tempdir().unwrap();
        let cfg = cfg_with_gate(proj.path());
        let result = bless_in(&cfg, &store_path, "t2");

        // Restore perms so tempdir cleanup can remove the file regardless of
        // assertion outcome below.
        let mut perms = fs::metadata(&store_path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&store_path, perms).unwrap();

        assert!(
            result.is_err(),
            "bless_in must fail loudly on a non-parse read error, not silently clobber"
        );
        let after = fs::read(&store_path).unwrap();
        assert_eq!(
            before, after,
            "store bytes must be unchanged after a failed bless"
        );
    }

    #[test]
    fn classify_store_read_permission_denied_propagates_err() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("trust.json");
        let err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        assert!(classify_store_read(&store_path, Err(err)).is_err());
    }

    #[test]
    fn classify_store_read_not_found_is_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("trust.json");
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let store = classify_store_read(&store_path, Err(err)).unwrap();
        assert!(store.entries.is_empty());
    }

    #[test]
    fn classify_store_read_invalid_json_is_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("trust.json");
        let store = classify_store_read(&store_path, Ok("{ not json".to_string())).unwrap();
        assert!(store.entries.is_empty());
    }
}
