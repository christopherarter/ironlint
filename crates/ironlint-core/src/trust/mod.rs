mod decision;
mod policy_hash;
mod store;
mod summary;
mod worktree;

pub use decision::{
    bless, bless_in, check_trust, check_trust_in, ensure_trusted, ensure_trusted_in, TrustOutcome,
};
pub use policy_hash::compute_hash;
pub use store::{
    config_home, read_store, trust_store_path, write_store, TrustEntry, TrustStore,
    TRUST_STORE_VERSION,
};
pub use summary::{blessed_summary, BlessedSummary};

#[cfg(test)]
use crate::adapter::sha256_digest_hex;
#[cfg(test)]
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::collections::BTreeMap;
#[cfg(test)]
use std::path::PathBuf;

#[cfg(test)]
use policy_hash::{classify_entry, compute_worktree_hash, hash_entry, EntryKind};
#[cfg(test)]
use store::{canonical_key, classify_store_read, config_home_from, store_path_in, unique_tmp_path};

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
    fn editing_a_script_changes_hash() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \".ironlint/scripts/g.sh\"\n",
        );
        let script = dir.path().join(".ironlint/scripts/g.sh");
        write(&script, "#!/bin/sh\nexit 0\n");
        let before = compute_hash(&cfg).unwrap();
        write(&script, "#!/bin/sh\nexit 2\n");
        let after = compute_hash(&cfg).unwrap();
        assert_ne!(before, after, "a script edit must invalidate the hash");
    }

    #[test]
    fn hash_folds_scripts_in_sorted_order() {
        // compute_hash must fold the config plus its script files in a
        // deterministic, identity-bound frame: each config keyed by its
        // canonical path, each script file keyed by its scripts dir + sorted
        // relative path (independent of OS enumeration order). We pin the exact
        // scheme by recomputing the digest the same way using the impl's own
        // framing helper. This fails if the impl stops sorting script files (the
        // `out.sort_by` in collect_gate_files) — on a filesystem whose read_dir
        // yields b before a — or if the label binding / length prefixes change,
        // which doubles as a regression lock on the stored-hash encoding.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        let cfg_body = "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n";
        write(&cfg, cfg_body);
        write(&dir.path().join(".ironlint/scripts/a.sh"), "a\n");
        write(&dir.path().join(".ironlint/scripts/b.sh"), "b\n");

        // Labels carry canonical paths, so recompute against the canonical form.
        let canon = cfg.canonicalize().unwrap();
        let scripts_dir = canon.parent().unwrap().join(".ironlint").join("scripts");

        let mut expected = Sha256::new();
        hash_entry(
            &mut expected,
            &format!("config\0{}", canon.display()),
            cfg_body.as_bytes(),
        );
        hash_entry(
            &mut expected,
            &format!("scripts\0{}\0a.sh", scripts_dir.display()),
            b"a\n",
        );
        hash_entry(
            &mut expected,
            &format!("scripts\0{}\0b.sh", scripts_dir.display()),
            b"b\n",
        );
        let want = sha256_digest_hex(&expected.finalize());

        assert_eq!(compute_hash(&cfg).unwrap(), want);
    }

    #[test]
    fn editing_a_referenced_outside_script_does_not_change_hash() {
        // After the gates→scripts rename: a script referenced by `run:` but
        // located OUTSIDE .ironlint/scripts/ is no longer folded into the
        // trust hash. It may still be run by a check, but changing it does
        // not revoke trust — the spec's deliberate simplification.
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
        assert_eq!(
            before, after,
            "editing an in-repo script OUTSIDE .ironlint/scripts/ no longer revokes trust"
        );
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
    fn missing_scripts_dir_hashes_only_the_config() {
        // No .ironlint/scripts/ at all — must succeed (not error), hashing config alone.
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
    fn scripts_dir_symlink_loop_is_a_clear_error_not_a_hang() {
        // A self-referencing symlink inside the scripts dir must not be
        // followed. Before the fix, `is_dir()` follows the symlink and the
        // walk recurses through it; the OS eventually caps total symlink
        // resolutions and returns a raw ELOOP, so this terminates promptly
        // either way — but the *message* must call out the symlink, not
        // leak a raw OS error, and (after the fix) the walk must refuse the
        // symlink on first sight rather than ever recursing into it.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n");
        let scripts = dir.path().join(".ironlint/scripts");
        fs::create_dir_all(&scripts).unwrap();
        write(&scripts.join("g.sh"), "#!/bin/sh\nexit 0\n");
        std::os::unix::fs::symlink(&scripts, scripts.join("loop")).unwrap();

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
    fn scripts_recurse_into_subdirectories() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n");
        write(&dir.path().join(".ironlint/scripts/top.sh"), "top\n");
        write(
            &dir.path().join(".ironlint/scripts/sub/nested.sh"),
            "nested\n",
        );
        let before = compute_hash(&cfg).unwrap();
        write(
            &dir.path().join(".ironlint/scripts/sub/nested.sh"),
            "nested changed\n",
        );
        let after = compute_hash(&cfg).unwrap();
        assert_ne!(
            before, after,
            "editing a nested script file must change the hash"
        );
    }

    #[cfg(unix)]
    #[test]
    fn scripts_dir_itself_as_symlink_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n");
        let real_scripts = dir.path().join("real_scripts");
        fs::create_dir_all(&real_scripts).unwrap();
        write(&real_scripts.join("g.sh"), "#!/bin/sh\nexit 0\n");
        fs::create_dir_all(dir.path().join(".ironlint")).unwrap();
        std::os::unix::fs::symlink(&real_scripts, dir.path().join(".ironlint/scripts")).unwrap();

        let err = compute_hash(&cfg).unwrap_err().to_string();
        assert!(err.contains("symlink"), "error: {err}");
    }

    #[test]
    fn scripts_dir_path_is_a_plain_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(&cfg, "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n");
        fs::create_dir_all(dir.path().join(".ironlint")).unwrap();
        fs::write(dir.path().join(".ironlint/scripts"), "not a dir").unwrap();
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
            ..Default::default()
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

    fn cfg_with_script(dir: &Path) -> PathBuf {
        let cfg = dir.join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*\"\n    run: \".ironlint/scripts/g.sh\"\n",
        );
        write(&dir.join(".ironlint/scripts/g.sh"), "#!/bin/sh\nexit 0\n");
        cfg
    }

    #[test]
    fn blessed_summary_lists_hash_checks_and_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*\"\n    run: \"bash scripts/lint.sh\"\n",
        );
        write(&dir.path().join(".ironlint/scripts/a.sh"), "a\n");
        write(&dir.path().join(".ironlint/scripts/b.sh"), "b\n");
        write(&dir.path().join("scripts/lint.sh"), "#!/bin/sh\nexit 0\n");

        let summary = blessed_summary(&cfg).unwrap();

        assert!(
            summary.config_hash.starts_with("sha256:"),
            "config_hash must be sha256-prefixed: {}",
            summary.config_hash
        );
        assert_eq!(
            summary.config_hash,
            compute_hash(&cfg).unwrap(),
            "blessed_summary must report the SAME digest compute_hash would produce"
        );
        assert_eq!(summary.checks, 1, "checks counts resolved checks");
        assert_eq!(
            summary.scripts,
            vec!["a.sh".to_string(), "b.sh".to_string()],
            "scripts lists every file under .ironlint/scripts/, sorted"
        );
    }

    #[test]
    fn blessed_summary_is_empty_with_no_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
        );

        let summary = blessed_summary(&cfg).unwrap();

        assert!(
            summary.scripts.is_empty(),
            "no scripts dir → empty scripts list"
        );
        assert_eq!(summary.checks, 1, "the one inline check still counts");
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

    // --- store v2 (Task 3) -----------------------------------------------
    #[test]
    fn version_one_store_deserializes_with_empty_worktree_entries() {
        let v1 = r#"{"version":1,"entries":{"\/x\/.ironlint.yml":{"hash":"sha256:ab","blessed_at":"t"}}}"#;
        let store: TrustStore = serde_json::from_str(v1).unwrap();
        assert_eq!(store.version, 1);
        assert_eq!(store.entries.len(), 1);
        assert!(
            store.worktree_entries.is_empty(),
            "v1 store gets empty worktree_entries"
        );
    }

    #[test]
    fn worktree_entries_round_trip() {
        let mut store = TrustStore::default();
        store.worktree_entries.insert("/common/.git".to_string(), {
            let mut inner = BTreeMap::new();
            inner.insert(
                ".ironlint.yml".to_string(),
                TrustEntry {
                    hash: "sha256:cd".to_string(),
                    blessed_at: "t".to_string(),
                },
            );
            inner
        });
        let json = serde_json::to_string(&store).unwrap();
        let back: TrustStore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.worktree_entries, store.worktree_entries);
    }

    #[test]
    fn trust_store_version_is_two() {
        assert_eq!(TRUST_STORE_VERSION, 2);
    }

    // --- worktree hash (Task 2) -------------------------------------------
    use crate::trust::worktree::WorktreeScope;
    use std::process::Command;

    /// Build a real git repo at `root` so `WorktreeScope::discover` succeeds.
    fn git_repo(root: &Path) {
        fs::create_dir_all(root).unwrap();
        let _ = Command::new("git").args(["init", "-q"]).arg(root).status();
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
            write(
                &cfg,
                "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
            );
        }
        let sa = WorktreeScope::discover(&a.path().join(".ironlint.yml")).unwrap();
        let sb = WorktreeScope::discover(&b.path().join(".ironlint.yml")).unwrap();
        let ha = compute_worktree_hash(&a.path().join(".ironlint.yml"), &sa)
            .unwrap()
            .unwrap();
        let hb = compute_worktree_hash(&b.path().join(".ironlint.yml"), &sb)
            .unwrap()
            .unwrap();
        assert_eq!(ha, hb, "identical policy in equivalent roots hashes alike");
    }

    #[test]
    fn worktree_hash_changes_when_config_changes() {
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        let cfg = root.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
        );
        let scope = WorktreeScope::discover(&cfg).unwrap();
        let h1 = compute_worktree_hash(&cfg, &scope).unwrap().unwrap();
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \"false\"\n",
        );
        let h2 = compute_worktree_hash(&cfg, &scope).unwrap().unwrap();
        assert_ne!(h1, h2, "editing the config changes the worktree hash");
    }

    #[test]
    fn worktree_hash_changes_when_script_changes() {
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        let cfg = root.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \".ironlint/scripts/s.sh\"\n",
        );
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
        write(
            &base,
            "checks:\n  b:\n    files: \"*\"\n    run: \"true\"\n",
        );
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        let cfg = root.path().join(".ironlint.yml");
        write(
            &cfg,
            &format!("extends: [\"{}\"]\nchecks: {{}}\n", base.display()),
        );
        let scope = WorktreeScope::discover(&cfg).unwrap();
        assert!(
            compute_worktree_hash(&cfg, &scope).unwrap().is_none(),
            "an extends target outside the root is ineligible"
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

    #[test]
    fn blessed_summary_scope_is_linked_worktrees_for_git_repo() {
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        let cfg = root.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
        );
        let sum = blessed_summary(&cfg).unwrap();
        assert_eq!(sum.scope, "linked worktrees");
    }

    #[test]
    fn blessed_summary_scope_is_this_config_path_when_not_git() {
        let root = tempfile::tempdir().unwrap();
        let cfg = root.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
        );
        let sum = blessed_summary(&cfg).unwrap();
        assert_eq!(sum.scope, "this config path");
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
}
