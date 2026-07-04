use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `"sha256:<lowercase-hex>"` of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("sha256:{:x}", h.finalize())
}

/// Write `bytes` to `path` atomically (temp sibling + rename), creating parents.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("ironlint")
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

/// Copy `path` to `<path>.bak` only if the file exists and no backup exists yet.
pub fn backup_once(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let bak = path.with_extension(format!(
        "{}.bak",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    if bak.exists() {
        return Ok(());
    }
    std::fs::copy(path, &bak).with_context(|| format!("backing up {}", path.display()))?;
    Ok(())
}

/// Per-harness integrity record, written beside the materialized artifacts.
///
/// No `#[serde(deny_unknown_fields)]`: sidecars written by older binaries carry
/// a `"version"` key that no longer maps to a field. Serde ignores it on read,
/// so those files still deserialize (staleness is now derived from the recorded
/// hashes, not a version counter).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterSidecar {
    /// filename -> "sha256:<hex>"
    pub files: BTreeMap<String, String>,
}

/// `<dir>/.ironlint-adapter.json`.
pub fn sidecar_path(dir: &Path) -> PathBuf {
    dir.join(".ironlint-adapter.json")
}

pub fn write_sidecar(dir: &Path, sidecar: &AdapterSidecar) -> Result<()> {
    let json =
        serde_json::to_string_pretty(sidecar).with_context(|| "serializing adapter sidecar")?;
    atomic_write(&sidecar_path(dir), json.as_bytes())
}

pub fn read_sidecar(dir: &Path) -> Result<Option<AdapterSidecar>> {
    match std::fs::read_to_string(sidecar_path(dir)) {
        Ok(s) => Ok(Some(
            serde_json::from_str(&s).with_context(|| "parsing adapter sidecar")?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("reading adapter sidecar"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn sha256_hex_is_prefixed_and_stable() {
        assert_eq!(
            sha256_hex(b"hello"),
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn atomic_write_creates_parents_and_content() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a/b/c.sh");
        atomic_write(&p, b"#!/bin/sh\n").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"#!/bin/sh\n");
    }

    #[test]
    fn backup_once_preserves_first_original_only() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("settings.json");
        std::fs::write(&p, b"original").unwrap();
        backup_once(&p).unwrap();
        std::fs::write(&p, b"changed").unwrap();
        backup_once(&p).unwrap(); // must NOT overwrite the pristine backup
        assert_eq!(
            std::fs::read(p.with_extension("json.bak")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn backup_once_noop_when_file_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("missing.json");
        backup_once(&p).unwrap();
        assert!(!p.with_extension("json.bak").exists());
    }

    #[test]
    fn sidecar_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let mut files = BTreeMap::new();
        files.insert("hook.sh".to_string(), "sha256:abc".to_string());
        let sc = AdapterSidecar { files };
        write_sidecar(tmp.path(), &sc).unwrap();
        let back = read_sidecar(tmp.path()).unwrap().unwrap();
        assert_eq!(back.files.get("hook.sh").unwrap(), "sha256:abc");
    }

    #[test]
    fn read_sidecar_ignores_legacy_version_key() {
        // Back-compat: a sidecar written by a pre-5.21 binary carries a
        // `"version"` key with no matching field. It must still deserialize
        // (unknown key ignored) so an existing install keeps its integrity data.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            sidecar_path(tmp.path()),
            br#"{"version":7,"files":{"hook.sh":"sha256:abc"}}"#,
        )
        .unwrap();
        let back = read_sidecar(tmp.path()).unwrap().unwrap();
        assert_eq!(back.files.get("hook.sh").unwrap(), "sha256:abc");
    }

    #[test]
    fn read_sidecar_absent_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_sidecar(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn read_sidecar_malformed_json_is_err() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(sidecar_path(tmp.path()), b"{ not json").unwrap();
        assert!(read_sidecar(tmp.path()).is_err());
    }

    #[test]
    fn read_sidecar_io_error_is_err() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(sidecar_path(tmp.path())).unwrap();
        assert!(read_sidecar(tmp.path()).is_err());
    }
}
