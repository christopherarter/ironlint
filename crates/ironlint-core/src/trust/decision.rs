use anyhow::{Context, Result};
use std::path::Path;

use super::policy_hash::{compute_hash, compute_worktree_hash};
use super::store::{
    acquire_store_lock, canonical_key, read_store, read_store_for_bless, trust_store_path,
    write_store, TrustEntry, TrustStore, TRUST_STORE_VERSION,
};
use super::worktree::WorktreeScope;

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
    /// scripts dir contains a symlink, etc. This is a structural config
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
    let Some(scope) = WorktreeScope::discover(config_path) else {
        return false;
    };
    let Ok(Some(wt_hash)) = compute_worktree_hash(config_path, &scope) else {
        return false;
    };
    store
        .worktree_entries
        .get(scope.common_dir.to_string_lossy().as_ref())
        .and_then(|m| m.get(&scope.config_rel))
        .is_some_and(|e| e.hash == wt_hash)
}

/// Step 6: read-only legacy fallback. For each old direct entry whose path
/// still resolves to the SAME (common_dir, config_rel) with an unchanged
/// direct hash AND a matching worktree hash, accept. Never writes.
fn legacy_fallback_trusted(config_path: &Path, store: &TrustStore) -> bool {
    let Some(scope) = WorktreeScope::discover(config_path) else {
        return false;
    };
    let Ok(Some(current_wt_hash)) = compute_worktree_hash(config_path, &scope) else {
        return false;
    };
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
    let Ok(canon) = candidate.canonicalize() else {
        return false;
    };
    let Some(cand_scope) = WorktreeScope::discover(&canon) else {
        return false;
    };
    // (2) same common dir + config_rel
    if cand_scope.common_dir != scope.common_dir || cand_scope.config_rel != scope.config_rel {
        return false;
    }
    // (3) recomputed direct hash still equals the stored entry
    let Ok(direct) = compute_hash(&canon) else {
        return false;
    };
    if direct != entry.hash {
        return false;
    }
    // (4) recomputed worktree hash equals the current worktree hash
    let Ok(Some(cand_wt)) = compute_worktree_hash(&canon, &cand_scope) else {
        return false;
    };
    cand_wt == current_wt_hash
}

/// Verify `config_path` (and its scripts) match a blessed entry in the
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
    write_worktree_entry(&mut store, config_path, now);
    write_store(store_path, &store)
}

/// If `config_path` has an eligible worktree scope, record its worktree hash
/// (identical content, root-relative labels) in `worktree_entries`. Best-effort:
/// any discovery/hash failure is swallowed — blessing still succeeds with the
/// direct entry already written by the caller.
fn write_worktree_entry(store: &mut TrustStore, config_path: &Path, now: &str) {
    let Some(scope) = WorktreeScope::discover(config_path) else {
        return;
    };
    let Ok(Some(wt_hash)) = compute_worktree_hash(config_path, &scope) else {
        return;
    };
    let common = scope.common_dir.to_string_lossy().to_string();
    store.worktree_entries.entry(common).or_default().insert(
        scope.config_rel,
        TrustEntry {
            hash: wt_hash,
            blessed_at: now.to_string(),
        },
    );
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
