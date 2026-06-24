use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

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

/// Compute the trust hash of a config: sha256 over the config file bytes plus
/// every file under `<config-dir>/.hector/gates/` (sorted by relative path).
/// Returns `"sha256:<hex>"`.
pub fn compute_hash(config_path: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    let cfg_bytes =
        std::fs::read(config_path).with_context(|| format!("reading {}", config_path.display()))?;
    hash_entry(&mut hasher, "config", &cfg_bytes);

    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let gates_dir = config_dir.join(".hector").join("gates");
    if gates_dir.is_dir() {
        for (rel, bytes) in collect_gate_files(&gates_dir)? {
            hash_entry(&mut hasher, &format!("gates/{rel}"), &bytes);
        }
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
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
            "gates:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
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
            "gates:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
        );
        let before = compute_hash(&cfg).unwrap();
        write(
            &cfg,
            "gates:\n  g:\n    files: \"*.rs\"\n    run: \"false\"\n",
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
            "gates:\n  g:\n    files: \"*.rs\"\n    run: \".hector/gates/g.sh\"\n",
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
        // compute_hash must fold gate files in sorted-relative-path order,
        // independent of OS enumeration order. We pin the exact scheme by
        // recomputing the expected digest with the files folded in sorted
        // order using the impl's own framing helper. This fails if the impl
        // ever stops sorting (the `out.sort_by` in collect_gate_files) — on a
        // filesystem whose read_dir yields b before a — or if the hashing
        // frame (labels / length prefixes) changes, which doubles as a
        // regression lock on the stored-hash encoding.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".hector.yml");
        let cfg_body = "gates:\n  g:\n    files: \"*\"\n    run: \"true\"\n";
        write(&cfg, cfg_body);
        write(&dir.path().join(".hector/gates/a.sh"), "a\n");
        write(&dir.path().join(".hector/gates/b.sh"), "b\n");

        let mut expected = Sha256::new();
        hash_entry(&mut expected, "config", cfg_body.as_bytes());
        hash_entry(&mut expected, "gates/a.sh", b"a\n");
        hash_entry(&mut expected, "gates/b.sh", b"b\n");
        let want = format!("sha256:{:x}", expected.finalize());

        assert_eq!(compute_hash(&cfg).unwrap(), want);
    }

    #[test]
    fn missing_gates_dir_hashes_only_the_config() {
        // No .hector/gates/ at all — must succeed (not error), hashing config alone.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".hector.yml");
        write(
            &cfg,
            "gates:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
        );
        assert!(compute_hash(&cfg).unwrap().starts_with("sha256:"));
    }
}
