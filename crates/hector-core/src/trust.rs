use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Feed one labeled blob into the hasher with length prefixes on both the
/// label and the content, so no two distinct (label, bytes) pairs can collide
/// by concatenation.
fn hash_entry(hasher: &mut Sha256, label: &str, bytes: &[u8]) {
    hasher.update((label.len() as u64).to_le_bytes());
    hasher.update(label.as_bytes());
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
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
        if path.is_dir() {
            collect_into(root, &path, out)?;
        } else {
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
    }
    Ok(())
}

/// Compute the trust hash of a config over its **entire `extends:` closure**.
///
/// Sha256 over every config file reachable from `config_path` (the root plus
/// every transitively-extended file) plus every file under each participating
/// config dir's `.hector/gates/`. Returns `"sha256:<hex>"`.
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
    // `.hector/gates/`. Dedup so a shared dir is never double-folded.
    let mut gate_dirs: Vec<PathBuf> = config_paths
        .iter()
        .map(|p| {
            p.parent()
                .unwrap_or_else(|| Path::new("."))
                .join(".hector")
                .join("gates")
        })
        .collect();
    gate_dirs.sort();
    gate_dirs.dedup();

    for gates_dir in &gate_dirs {
        if gates_dir.is_dir() {
            for (rel, bytes) in collect_gate_files(gates_dir)? {
                hash_entry(
                    &mut hasher,
                    &format!("gates\0{}\0{rel}", gates_dir.display()),
                    &bytes,
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
    config_home.join("hector").join("trust.json")
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

/// Write the store atomically: serialize to a sibling temp file, then rename.
pub fn write_store(path: &Path, store: &TrustStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(store)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
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
        _ => anyhow::bail!("config/gates not trusted — review and run `hector trust`"),
    }
}

/// Recompute the hash of `config_path` and write it to the store as blessed.
///
/// Validates the full `extends:` closure first (not just the local file) so a
/// config whose `extends:` target is missing or broken is never blessed.
pub fn bless_in(config_path: &Path, store_path: &Path, now: &str) -> Result<()> {
    crate::config::parse_file_with_extends(config_path)
        .context("refusing to trust a config that does not parse")?;
    let key = canonical_key(config_path)?;
    let hash = compute_hash(config_path)?;
    let mut store = read_store(store_path)?;
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
        let cfg = dir.path().join(".hector.yml");
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
        let cfg = dir.path().join(".hector.yml");
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
        let cfg = dir.path().join(".hector.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \".hector/gates/g.sh\"\n",
        );
        let script = dir.path().join(".hector/gates/g.sh");
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
        let cfg = dir.path().join(".hector.yml");
        let cfg_body = "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n";
        write(&cfg, cfg_body);
        write(&dir.path().join(".hector/gates/a.sh"), "a\n");
        write(&dir.path().join(".hector/gates/b.sh"), "b\n");

        // Labels carry canonical paths, so recompute against the canonical form.
        let canon = cfg.canonicalize().unwrap();
        let gates_dir = canon.parent().unwrap().join(".hector").join("gates");

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
        let cfg = dir.path().join(".hector.yml");
        write(&cfg, "extends: [\"./nope.yml\"]\nchecks: {}\n");
        let err = compute_hash(&cfg).unwrap_err().to_string();
        assert!(
            err.contains("extends closure"),
            "error should name the closure resolution: {err}"
        );
    }

    #[test]
    fn missing_gates_dir_hashes_only_the_config() {
        // No .hector/gates/ at all — must succeed (not error), hashing config alone.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".hector.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
        );
        assert!(compute_hash(&cfg).unwrap().starts_with("sha256:"));
    }

    #[test]
    fn store_path_joins_under_config_home() {
        let p = store_path_in(Path::new("/home/u/.config"));
        assert_eq!(p, Path::new("/home/u/.config/hector/trust.json"));
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
            "/abs/.hector.yml".to_string(),
            TrustEntry {
                hash: "sha256:abc".into(),
                blessed_at: "2026-06-24T00:00:00Z".into(),
            },
        );
        write_store(&path, &store).unwrap();
        let back = read_store(&path).unwrap();
        assert_eq!(back.entries["/abs/.hector.yml"].hash, "sha256:abc");
        assert_eq!(
            back.entries["/abs/.hector.yml"].blessed_at,
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
        let cfg = dir.join(".hector.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*\"\n    run: \".hector/gates/g.sh\"\n",
        );
        write(&dir.join(".hector/gates/g.sh"), "#!/bin/sh\nexit 0\n");
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
            err.contains("hector trust"),
            "message must point at `hector trust`: {err}"
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
            &proj.path().join(".hector/gates/g.sh"),
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
        let cfg = proj.path().join(".hector.yml");
        write(&cfg, "schema_version: 2\nrules: {}\n"); // legacy → parser rejects
        assert!(bless_in(&cfg, &store.path().join("trust.json"), "t").is_err());
    }
}
