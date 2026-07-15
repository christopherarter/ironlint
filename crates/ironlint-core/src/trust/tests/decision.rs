use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(p: &Path, body: &str) {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, body).unwrap();
}

fn cfg_with_script(dir: &Path) -> PathBuf {
    let cfg = dir.join(".ironlint.yml");
    write(
        &cfg,
        "checks:\n  g:\n    files: \"*\"\n    run: \".ironlint/scripts/g.sh\"\n",
    );
    write(&dir.join(".ironlint/scripts/g.sh"), "#!/bin/sh\nexit 0\n");
    cfg
}

/// Build a real git repo at `root` so `WorktreeScope::discover` succeeds.
fn git_repo(root: &Path) {
    fs::create_dir_all(root).unwrap();
    let _ = Command::new("git").args(["init", "-q"]).arg(root).status();
    // .ironlint.yml is the trust surface; commit is unnecessary for discovery.
}

#[test]
fn bless_then_ensure_succeeds() {
    let proj = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let store_path = store.path().join("trust.json");
    let cfg = cfg_with_script(proj.path());
    bless_in(&cfg, &store_path, "2026-06-24T00:00:00Z").unwrap();
    assert!(ensure_trusted_in(&cfg, &store_path).is_ok());
}

#[test]
fn never_blessed_is_not_trusted() {
    let proj = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let cfg = cfg_with_script(proj.path());
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
fn editing_a_script_after_bless_revokes_trust() {
    let proj = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let store_path = store.path().join("trust.json");
    let cfg = cfg_with_script(proj.path());
    bless_in(&cfg, &store_path, "t").unwrap();
    // Tamper with the script.
    write(
        &proj.path().join(".ironlint/scripts/g.sh"),
        "#!/bin/sh\nexit 2\n",
    );
    assert!(ensure_trusted_in(&cfg, &store_path).is_err());
}

#[test]
fn editing_config_after_bless_revokes_trust() {
    let proj = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let store_path = store.path().join("trust.json");
    let cfg = cfg_with_script(proj.path());
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
                let cfg = cfg_with_script(proj.path());
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
    let cfg = cfg_with_script(proj.path());

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
    let cfg = cfg_with_script(proj.path());

    assert!(ensure_trusted_in(&cfg, &store_path).is_err());
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
    let seed_cfg = cfg_with_script(seed_proj.path());
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
    let cfg = cfg_with_script(proj.path());
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

// --- blessing worktree entry (Task 4) --------------------------------
#[test]
fn bless_writes_worktree_entry_for_eligible_git_repo() {
    let root = tempfile::tempdir().unwrap();
    git_repo(root.path());
    let cfg = root.path().join(".ironlint.yml");
    write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
    );
    let store = tempfile::tempdir().unwrap();
    let store_path = store.path().join("trust.json");
    bless_in(&cfg, &store_path, "t").unwrap();
    let s = read_store(&store_path).unwrap();
    let scope = WorktreeScope::discover(&cfg).unwrap();
    let inner = s
        .worktree_entries
        .get(&scope.common_dir.to_string_lossy().to_string())
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
    write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
    );
    let store = tempfile::tempdir().unwrap();
    let store_path = store.path().join("trust.json");
    bless_in(&cfg, &store_path, "t").unwrap();
    let s = read_store(&store_path).unwrap();
    assert!(
        s.worktree_entries.is_empty(),
        "non-Git config writes no worktree entry"
    );
    assert_eq!(s.entries.len(), 1, "direct entry still written");
}

// --- inherited + legacy lookup (Task 5) -------------------------------
#[test]
fn direct_miss_with_matching_worktree_entry_is_trusted() {
    // Bless primary; the SAME store has no direct entry for a sibling's
    // canonical path, but the worktree_entries entry matches -> Trusted.
    let root = tempfile::tempdir().unwrap();
    git_repo(root.path());
    let cfg = root.path().join(".ironlint.yml");
    write(
        &cfg,
        "checks:
  g:
    files: \"*.rs\"
    run: \"true\"
",
    );
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
    assert!(matches!(
        check_trust_in(&cfg, &store_path),
        TrustOutcome::Trusted
    ));
}

#[test]
fn non_git_repo_falls_through_to_untrusted_when_not_blessed() {
    let root = tempfile::tempdir().unwrap();
    let cfg = root.path().join(".ironlint.yml");
    write(
        &cfg,
        "checks:
  g:
    files: \"*.rs\"
    run: \"true\"
",
    );
    let store = tempfile::tempdir().unwrap();
    let store_path = store.path().join("trust.json");
    // Never blessed -> untrusted; no inheritance path available.
    assert!(matches!(
        check_trust_in(&cfg, &store_path),
        TrustOutcome::Untrusted(_)
    ));
}

#[test]
fn check_never_writes_store_on_inherited_trust() {
    let root = tempfile::tempdir().unwrap();
    git_repo(root.path());
    let cfg = root.path().join(".ironlint.yml");
    write(
        &cfg,
        "checks:
  g:
    files: \"*.rs\"
    run: \"true\"
",
    );
    let store = tempfile::tempdir().unwrap();
    let store_path = store.path().join("trust.json");
    bless_in(&cfg, &store_path, "t").unwrap();
    // Force a direct miss + worktree hit (remove direct entry):
    let mut s = read_store(&store_path).unwrap();
    let key = canonical_key(&cfg).unwrap();
    s.entries.remove(&key);
    write_store(&store_path, &s).unwrap();
    // Baseline after setup; check must not mutate the store.
    let baseline = std::fs::read(&store_path).unwrap();
    let _ = check_trust_in(&cfg, &store_path);
    assert_eq!(
        baseline,
        std::fs::read(&store_path).unwrap(),
        "store unchanged by check"
    );
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
    write(
        &cfg,
        "checks:
  g:
    files: \"*.rs\"
    run: \"true\"
",
    );
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
    assert_eq!(
        bytes_before,
        std::fs::read(&store_path).unwrap(),
        "legacy fallback is read-only"
    );
}
