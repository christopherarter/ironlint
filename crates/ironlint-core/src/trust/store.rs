use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const TRUST_STORE_VERSION: u32 = 2;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustStore {
    /// Schema version. A defaulted (never-written) store has `0`; a store
    /// written by `bless` carries `TRUST_STORE_VERSION`. Trust decisions key
    /// off per-entry hashes, not this field — it exists for future migrations.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustEntry {
    pub hash: String,
    pub blessed_at: String,
}

/// `$XDG_CONFIG_HOME` (if set and non-empty) else `$HOME/.config`. Pure
/// resolver split out from the env read so it is testable without mutating
/// process env.
pub(super) fn config_home_from(xdg: Option<String>, home: Option<String>) -> Option<PathBuf> {
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

pub(super) fn store_path_in(config_home: &Path) -> PathBuf {
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
pub(super) fn unique_tmp_path(path: &Path) -> PathBuf {
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
///
/// `cfg(unix)`: its only caller, `acquire_store_lock` below, is itself
/// unix-only (see that fn's doc comment) — gating this the same way avoids
/// a `dead_code` warning (which `-D warnings` turns into a hard error) on
/// the Windows compile-only CI leg (Task 3.7 / C2).
#[cfg(unix)]
fn lock_path(store_path: &Path) -> PathBuf {
    store_path.with_extension("lock")
}

/// RAII guard for the exclusive store lock: dropping it releases the flock
/// (closing the underlying fd releases a POSIX `flock`, so an explicit
/// `unlock` call isn't needed here — unlike `telemetry::append`, which
/// unlocks eagerly to shrink its critical section).
#[cfg(unix)]
pub(super) struct StoreLock {
    _file: std::fs::File,
}

#[cfg(unix)]
pub(super) fn acquire_store_lock(store_path: &Path) -> Result<StoreLock> {
    use fs4::FileExt;
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
    FileExt::lock(&file).with_context(|| format!("locking {}", store_path.display()))?;
    Ok(StoreLock { _file: file })
}

/// No file-locking primitive is wired up for non-unix targets yet (tracked
/// alongside the broader Windows-support gap); blessing still works, just
/// without cross-process serialization.
#[cfg(not(unix))]
pub(super) struct StoreLock;

#[cfg(not(unix))]
pub(super) fn acquire_store_lock(store_path: &Path) -> Result<StoreLock> {
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
pub(super) fn classify_store_read(
    store_path: &Path,
    read: std::io::Result<String>,
) -> Result<TrustStore> {
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
pub(super) fn read_store_for_bless(store_path: &Path) -> Result<TrustStore> {
    classify_store_read(store_path, std::fs::read_to_string(store_path))
}

/// Canonical absolute path used as the store key for `config_path`.
pub(super) fn canonical_key(config_path: &Path) -> Result<String> {
    let canon = config_path
        .canonicalize()
        .with_context(|| format!("resolving {}", config_path.display()))?;
    Ok(canon.to_string_lossy().to_string())
}
