use crate::config::Config;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
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

/// Split a shell command string into candidate path tokens: whitespace AND a
/// liberal set of shell metacharacters are all treated as separators. This is
/// deliberately not a real shell parser — it is a conservative *extractor*
/// for the referenced-scripts hash fold (see [`referenced_repo_files`]):
/// false positives (a token that happens to coincide with a real file, but
/// isn't actually "the script") are harmless, so splitting too eagerly is
/// safe. Splitting on quote characters means a quoted path containing a
/// space (e.g. `'my script.sh'`) is not recovered as one token — a known,
/// acceptable gap for a conservative, inclusion-biased extractor.
fn tokenize_command(cmd: &str) -> Vec<String> {
    const METACHARS: &[char] = &[
        ';', '|', '&', '>', '<', '(', ')', '{', '}', '$', '`', '\'', '"', '=', '*',
    ];
    cmd.split(|c: char| c.is_whitespace() || METACHARS.contains(&c))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// `true` iff `path` is a regular file **without following a symlink** at
/// the final component — mirrors the guard [`classify_entry`] applies to the
/// gates walk, but here a non-match is a silent skip rather than a hard
/// error: most command tokens are not paths at all, so treating every
/// unusual token as fatal would make `compute_hash` fail on ordinary
/// configs. A symlink-referenced script is therefore a residual gap: its
/// target is not covered by this hash, so it can be swapped without
/// revoking trust.
fn is_regular_file_no_follow(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_file())
        .unwrap_or(false)
}

/// Resolve `token` against `root` and return its repo-relative (`/`-joined)
/// path iff it names an existing, in-repo, regular, non-symlink file that
/// isn't already under one of `gate_dirs` (those are folded once by the
/// caller's separate gates walk — double-folding them would change the hash
/// of every config that references a gate script via `run:`, breaking the
/// "no-scripts/gates-only hash is unchanged" invariant). `None` for anything
/// that doesn't resolve, escapes `root`, isn't a regular file, or duplicates
/// a gate file.
fn candidate_repo_relative(token: &str, root: &Path, gate_dirs: &[PathBuf]) -> Option<String> {
    // `Path::join` with an absolute `token` discards `root` entirely (per
    // `std::path::PathBuf::push` semantics), which is exactly what we want:
    // an absolute token still gets canonicalized and containment-checked
    // below, so an absolute in-repo path is included and an absolute
    // out-of-repo path is excluded by the `starts_with` check either way.
    let joined = root.join(token);
    // Check symlink-ness on the JOINED (pre-canonicalize) path, not the
    // canonicalized one: `canonicalize()` fully dereferences symlinks
    // (that's its job), so by the time a path is canonical it can never
    // itself "be a symlink" — checking after canonicalizing would silently
    // follow the very symlinks this guard exists to skip. `symlink_metadata`
    // here still resolves any symlinked *intermediate* directories (only the
    // final path component is left un-followed), matching `classify_entry`'s
    // guard elsewhere in this file.
    if !is_regular_file_no_follow(&joined) {
        return None;
    }
    let canon = joined.canonicalize().ok()?;
    if !canon.starts_with(root) {
        return None;
    }
    if gate_dirs.iter().any(|g| canon.starts_with(g)) {
        return None;
    }
    let rel = canon.strip_prefix(root).ok()?;
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Extract every in-repo script referenced by any check's `run` or
/// `steps[].run`, resolved against `root`, excluding files already under
/// `gate_dirs`. Returns `(repo-relative path, bytes)` pairs sorted and
/// deduped by relative path, ready to fold into the trust hash.
///
/// Conservative by design (see [`tokenize_command`] and
/// [`candidate_repo_relative`]): err toward inclusion for anything that
/// resolves to a real in-repo regular file, since a missed script (false
/// negative) is the actual danger — a coincidental match (false positive)
/// only adds harmless extra bytes to the hash.
fn referenced_repo_files(
    cfg: &Config,
    root: &Path,
    gate_dirs: &[PathBuf],
) -> Result<Vec<(String, Vec<u8>)>> {
    let mut rels: BTreeSet<String> = BTreeSet::new();
    for check in cfg.checks.values() {
        for step in check.effective_steps() {
            for token in tokenize_command(&step.run) {
                if let Some(rel) = candidate_repo_relative(&token, root, gate_dirs) {
                    rels.insert(rel);
                }
            }
        }
    }
    let mut out = Vec::with_capacity(rels.len());
    for rel in rels {
        let path = root.join(&rel);
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        out.push((rel, bytes));
    }
    Ok(out)
}

/// Fold every in-repo script referenced by the merged config's checks into
/// `hasher`, under the `run-ref\0<repo-relative-path>` label namespace (kept
/// distinct from `config\0` and `gates\0` so no cross-namespace collision is
/// possible). `root` is the **primary** config's directory — checks run with
/// that as their cwd regardless of which `extends:`-inherited file defined
/// them, so that's what `run:` tokens resolve against.
///
/// Resolution failures here fold nothing rather than erroring: an
/// unparseable/unresolvable config runs no checks at all, so there is no
/// `run:` string to reference a script from and thus no RCE surface from
/// skipping this step — the config's own bytes are already folded by the
/// caller regardless. This mirrors [`compute_hash`]'s existing behavior of
/// never erroring on a config it could previously hash by bytes alone.
fn fold_referenced_scripts(
    hasher: &mut Sha256,
    config_path: &Path,
    gate_dirs: &[PathBuf],
) -> Result<()> {
    let Ok(canon_config) = config_path.canonicalize() else {
        return Ok(());
    };
    let root = canon_config
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let Ok(merged) = crate::config::extends::resolve(config_path) else {
        return Ok(());
    };
    for (rel, bytes) in referenced_repo_files(&merged, &root, gate_dirs)? {
        hash_entry(hasher, &format!("run-ref\0{rel}"), &bytes);
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

    // Fold in-repo scripts referenced by any check's run/steps that aren't
    // already covered by the gates loop above — closes the post-bless
    // script-swap gap (Task 3.3 / Finding C6): `run: "bash scripts/lint.sh"`
    // used to be trusted by its command *string* only, so scripts/lint.sh
    // could be rewritten after blessing and run on the next agent edit with
    // no re-bless. See `fold_referenced_scripts` for the leniency rationale.
    fold_referenced_scripts(&mut hasher, config_path, &gate_dirs)?;

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

/// Outcome of a trust-enforcement attempt against a specific store.
///
/// Kept distinct so callers (concretely: `ironlint-cli`'s `check` command,
/// Task 3.2 / Finding C3) can map them to different exit codes — an
/// untrusted or tampered config must be surfaced loudly (exit 4, and
/// pre-write adapters block), while a config the trust layer can't even
/// *evaluate* is a structural config problem the engine's own `load()` will
/// report a moment later on its own terms, so it keeps the ordinary
/// config-error exit code.
///
/// `ensure_trusted`/`ensure_trusted_in` collapse both non-`Trusted` variants
/// back into a plain `Err` — every existing caller that only cares "is this
/// trusted, yes or no" (`doctor`, the trust_extends integration tests, this
/// module's own unit tests) keeps working unchanged.
pub enum TrustOutcome {
    Trusted,
    /// The config's hash resolved fine but doesn't match (or has no) entry
    /// in the store — genuinely untrusted/tampered — OR the trust store
    /// itself couldn't be read (corrupt/unreadable): either way we cannot
    /// answer "is this trusted?" in the affirmative, so fail closed and
    /// surface it loudly.
    Untrusted(anyhow::Error),
    /// The trust hash could not be *computed* at all — the config (or an
    /// `extends:` target) doesn't parse, a referenced path is missing, a
    /// gates dir contains a symlink, etc. This is a structural config
    /// problem, not a trust decision: defer to the engine's own load error.
    Unverifiable(anyhow::Error),
}

/// Classify a trust-enforcement attempt against `store_path`. See
/// [`TrustOutcome`] for what each variant means and why the split exists.
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
    match store.entries.get(&key) {
        Some(entry) if entry.hash == expected => TrustOutcome::Trusted,
        _ => TrustOutcome::Untrusted(anyhow::anyhow!(
            "config/gates not trusted — review and run `ironlint trust`"
        )),
    }
}

/// Verify `config_path` (and its gate scripts) match a blessed entry in the
/// store at `store_path`. Fails closed with a fixed, actionable message.
///
/// Thin boolean wrapper over [`check_trust_in`] for callers that only need
/// "is this trusted" — see that function if you need to distinguish
/// untrusted from unverifiable (e.g. to pick an exit code).
pub fn ensure_trusted_in(config_path: &Path, store_path: &Path) -> Result<()> {
    match check_trust_in(config_path, store_path) {
        TrustOutcome::Trusted => Ok(()),
        TrustOutcome::Untrusted(e) | TrustOutcome::Unverifiable(e) => Err(e),
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

/// Thin wrapper: classify trust against the real out-of-repo store.
///
/// See [`check_trust_in`]/[`TrustOutcome`]. The outer `Result` only ever
/// fails when the store's location itself can't be resolved (no
/// `$XDG_CONFIG_HOME` / `$HOME`) — an environment problem, not a per-config
/// trust decision.
pub fn check_trust(config_path: &Path) -> Result<TrustOutcome> {
    Ok(check_trust_in(config_path, &trust_store_path()?))
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

    // --- Task 3.3: hash in-repo scripts referenced by run:/steps[].run ---
    // RED: today compute_hash never looks at a check's `run`/`steps[].run`
    // strings, so editing an in-repo script a check shells out to (but that
    // isn't under `.ironlint/gates/`) does not change the hash. Bless the
    // benign check, rewrite the script, and it runs on the next agent write
    // with no re-bless. These tests must fail before the fix.

    #[test]
    fn editing_a_referenced_script_changes_hash() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*\"\n    run: \"bash scripts/lint.sh\"\n",
        );
        let script = dir.path().join("scripts/lint.sh");
        write(&script, "#!/bin/sh\nexit 0\n");
        let before = compute_hash(&cfg).unwrap();
        write(&script, "#!/bin/sh\nexit 2\n");
        let after = compute_hash(&cfg).unwrap();
        assert_ne!(
            before, after,
            "editing an in-repo script referenced by `run:` must invalidate the hash"
        );
    }

    #[test]
    fn editing_an_unrelated_file_does_not_change_hash() {
        // Control: mutating a file that is NOT referenced by any check's
        // run/steps must leave the hash untouched.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*\"\n    run: \"bash scripts/lint.sh\"\n",
        );
        write(&dir.path().join("scripts/lint.sh"), "#!/bin/sh\nexit 0\n");
        write(&dir.path().join("README"), "hello\n");
        let before = compute_hash(&cfg).unwrap();
        write(&dir.path().join("README"), "goodbye\n");
        let after = compute_hash(&cfg).unwrap();
        assert_eq!(
            before, after,
            "editing an unrelated file must not change the trust hash"
        );
    }

    #[test]
    fn referenced_script_in_steps_run_changes_hash() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*\"\n    steps:\n      - run: \"true\"\n      - run: \"bash scripts/lint.sh\"\n",
        );
        let script = dir.path().join("scripts/lint.sh");
        write(&script, "#!/bin/sh\nexit 0\n");
        let before = compute_hash(&cfg).unwrap();
        write(&script, "#!/bin/sh\nexit 2\n");
        let after = compute_hash(&cfg).unwrap();
        assert_ne!(
            before, after,
            "a step's run referencing a script must be scanned too, not just single-step `run:`"
        );
    }

    #[test]
    fn referenced_script_from_inherited_check_uses_primary_root() {
        // Checks run with cwd = the primary config's directory, even when the
        // check itself is defined in an extended (inherited) file. So an
        // inherited check's `run:` must resolve relative to the PRIMARY
        // root, and editing that script must still revoke trust.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.yml");
        write(
            &base,
            "checks:\n  b:\n    files: \"*\"\n    run: \"bash scripts/base.sh\"\n",
        );
        let cfg = dir.path().join(".ironlint.yml");
        write(&cfg, "extends: [\"./base.yml\"]\nchecks: {}\n");
        write(&dir.path().join("scripts/base.sh"), "#!/bin/sh\nexit 0\n");
        let before = compute_hash(&cfg).unwrap();
        write(&dir.path().join("scripts/base.sh"), "#!/bin/sh\nexit 2\n");
        let after = compute_hash(&cfg).unwrap();
        assert_ne!(
            before, after,
            "an inherited check's referenced script must be hashed relative to the primary root"
        );
    }

    #[test]
    fn referenced_out_of_repo_path_is_not_folded() {
        // A run: string that happens to name a file OUTSIDE the project root
        // (e.g. an absolute path to a sibling directory) must not be folded —
        // only in-repo files are in scope for this hash.
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("evil.sh");
        write(&outside_file, "#!/bin/sh\nexit 0\n");
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            &format!(
                "checks:\n  g:\n    files: \"*\"\n    run: \"bash {}\"\n",
                outside_file.display()
            ),
        );
        let before = compute_hash(&cfg).unwrap();
        write(&outside_file, "#!/bin/sh\nexit 2\n");
        let after = compute_hash(&cfg).unwrap();
        assert_eq!(
            before, after,
            "a script outside the project root must not be folded into the trust hash"
        );
    }

    #[cfg(unix)]
    #[test]
    fn referenced_script_symlink_is_skipped_not_followed() {
        // Regular files only (same guard as Task 2.3's gates walk) — a
        // symlink-referenced script is a documented residual gap: it is not
        // covered by this hash, so its target can be swapped without
        // revoking trust.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*\"\n    run: \"bash scripts/lint.sh\"\n",
        );
        let real = dir.path().join("real_lint.sh");
        write(&real, "#!/bin/sh\nexit 0\n");
        fs::create_dir_all(dir.path().join("scripts")).unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("scripts/lint.sh")).unwrap();

        let before = compute_hash(&cfg).unwrap();
        write(&real, "#!/bin/sh\nexit 2\n");
        let after = compute_hash(&cfg).unwrap();
        assert_eq!(
            before, after,
            "a symlink-referenced script is skipped (regular files only, not followed) — \
             this is the documented residual limitation"
        );
    }

    #[test]
    fn editing_a_referenced_script_after_bless_revokes_trust() {
        // End-to-end re-bless proof: bless a config whose check shells out to
        // an in-repo script, confirm it's trusted, tamper with the script,
        // and confirm `ensure_trusted_in` now fails closed until re-bless.
        let proj = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let store_path = store.path().join("trust.json");
        let cfg = proj.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*\"\n    run: \"bash scripts/lint.sh\"\n",
        );
        write(&proj.path().join("scripts/lint.sh"), "#!/bin/sh\nexit 0\n");

        bless_in(&cfg, &store_path, "t").unwrap();
        assert!(ensure_trusted_in(&cfg, &store_path).is_ok());

        write(&proj.path().join("scripts/lint.sh"), "#!/bin/sh\nexit 2\n");
        assert!(
            ensure_trusted_in(&cfg, &store_path).is_err(),
            "rewriting a referenced script after bless must revoke trust until re-bless"
        );

        // And re-blessing restores trust.
        bless_in(&cfg, &store_path, "t2").unwrap();
        assert!(ensure_trusted_in(&cfg, &store_path).is_ok());
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
