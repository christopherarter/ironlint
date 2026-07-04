use std::path::{Path, PathBuf};

/// Resolve the `--config` value to an actual file path. If `config` is the
/// default (`.ironlint.yml`) and doesn't exist in cwd, walk up the directory
/// tree (stopping at a `.git` directory or the filesystem root) looking for
/// one. If `config` was explicitly passed (even if it's the default name but
/// the caller intends a specific file), the caller is responsible — but clap's
/// default-value mechanism can't distinguish "user typed --config .ironlint.yml"
/// from "user omitted --config". So: walk-up applies unconditionally when the
/// literal path doesn't exist in cwd.
///
/// Returns the resolved path, or an error with an actionable message naming
/// `ironlint init` when no config is found in cwd or any parent.
pub fn resolve_config(config: &Path) -> Result<PathBuf, String> {
    if config.exists() {
        return Ok(config.to_path_buf());
    }
    let cwd = std::env::current_dir().map_err(|e| format!("resolving cwd: {e}"))?;
    resolve_config_with_cwd(config, &cwd)
}

fn resolve_config_with_cwd(config: &Path, cwd: &Path) -> Result<PathBuf, String> {
    // An absolute path is unambiguously explicit; don't fall back to a parent
    // directory. Only walk up for relative paths (the clap default `.ironlint.yml`
    // plus any explicit relative config name).
    if config.is_absolute() {
        return Err(format!(
            "no config found at {} — run `ironlint init`",
            config.display()
        ));
    }
    let name = config
        .file_name()
        .ok_or_else(|| format!("invalid config path: {}", config.display()))?;
    let mut dir: &Path = cwd;
    loop {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Ok(candidate);
        }
        // Stop at the repo boundary.
        if dir.join(".git").exists() {
            return Err(format!(
                "no `{}` found in {} or any parent — run `ironlint init`",
                name.to_string_lossy(),
                cwd.display()
            ));
        }
        let Some(parent) = dir.parent() else {
            return Err(format!(
                "no `{}` found in {} or any parent — run `ironlint init`",
                name.to_string_lossy(),
                cwd.display()
            ));
        };
        dir = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn relative_found_in_cwd() {
        let tmp = tempdir().unwrap();
        let cfg = tmp.path().join(".ironlint.yml");
        fs::write(&cfg, "checks:\n").unwrap();
        let resolved = resolve_config_with_cwd(Path::new(".ironlint.yml"), tmp.path()).unwrap();
        assert_eq!(
            resolved.file_name(),
            Some(std::ffi::OsStr::new(".ironlint.yml"))
        );
    }

    #[test]
    fn relative_found_in_parent() {
        let tmp = tempdir().unwrap();
        let cfg = tmp.path().join(".ironlint.yml");
        fs::write(&cfg, "checks:\n").unwrap();
        let subdir = tmp.path().join("src/nested");
        fs::create_dir_all(&subdir).unwrap();
        let resolved = resolve_config_with_cwd(Path::new(".ironlint.yml"), &subdir).unwrap();
        assert_eq!(
            resolved.canonicalize().unwrap(),
            cfg.canonicalize().unwrap()
        );
    }

    #[test]
    fn git_boundary_stops_walk() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let subdir = tmp.path().join("deep");
        fs::create_dir_all(&subdir).unwrap();
        let err = resolve_config_with_cwd(Path::new(".ironlint.yml"), &subdir).unwrap_err();
        assert!(err.contains("ironlint init"), "{err}");
        assert!(err.contains(".ironlint.yml"), "{err}");
    }

    #[test]
    fn absolute_missing_is_error() {
        let err = resolve_config_with_cwd(
            Path::new("/definitely/not/a/real/.ironlint.yml"),
            Path::new("/"),
        )
        .unwrap_err();
        assert!(err.contains("ironlint init"), "{err}");
    }

    #[test]
    fn invalid_empty_config_path_is_error() {
        let err = resolve_config_with_cwd(Path::new(""), Path::new("/")).unwrap_err();
        assert!(err.contains("invalid config path"), "{err}");
    }
}
