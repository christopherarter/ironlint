//! Harness onboarding: materialize Hector's hook into supported coding agents.
mod json_settings;
mod materialize;
mod ops;
mod registry;

pub use json_settings::{remove_from_hook_array, sync_hook_array, PatchResult};
pub use materialize::{
    atomic_write, backup_once, read_sidecar, sha256_hex, sidecar_path, write_sidecar,
    AdapterSidecar,
};
pub use ops::{install, status, uninstall, HarnessStatus, InstallOutcome, InstallResult};
pub use registry::{all_harnesses, JsonHookSpec, PluginSpec, SkillSpec, SKILL_NAME};

use std::path::PathBuf;

/// Bump when any embedded adapter artifact changes shape; drives doctor's
/// "outdated, re-run hector init" check.
pub const CURRENT_ADAPTER_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Local,
    Global,
}

/// Injectable environment so install/detect are testable without touching the
/// real `$HOME`.
#[derive(Debug, Clone)]
pub struct AdapterEnv {
    pub home: PathBuf,
    pub config_home: PathBuf,
    pub project_root: PathBuf,
}

impl AdapterEnv {
    /// Resolve from the process environment + a project root (cwd or `--dir`).
    pub fn from_process(project_root: PathBuf) -> anyhow::Result<Self> {
        Self::from_parts(
            std::env::var("HOME").ok(),
            crate::trust::config_home(),
            project_root,
        )
    }

    /// Pure resolver split out from the env read so the error arms are
    /// testable without mutating process env (mirrors `trust::config_home_from`).
    fn from_parts(
        home: Option<String>,
        config_home: Option<PathBuf>,
        project_root: PathBuf,
    ) -> anyhow::Result<Self> {
        let home = home
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("cannot resolve $HOME"))?;
        let config_home = config_home.ok_or_else(|| {
            anyhow::anyhow!("cannot resolve config home (set $XDG_CONFIG_HOME or $HOME)")
        })?;
        Ok(Self {
            home,
            config_home,
            project_root,
        })
    }
}

pub enum HarnessKind {
    JsonHook(JsonHookSpec),
    Plugin(PluginSpec),
}

pub struct Harness {
    pub name: &'static str,
    pub kind: HarnessKind,
    pub restart_hint: &'static str,
    pub skill: SkillSpec,
}

/// `<config_home>/hector/adapters` — sits beside the trust store.
pub fn adapters_dir(env: &AdapterEnv) -> PathBuf {
    env.config_home.join("hector").join("adapters")
}

/// `(harness-name, installed-on-this-machine?)` for every supported harness.
pub fn detect(env: &AdapterEnv) -> Vec<(&'static str, bool)> {
    all_harnesses()
        .into_iter()
        .map(|h| (h.name, registry::is_detected(&h, env)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_parts_builds_when_both_present() {
        let e = AdapterEnv::from_parts(
            Some("/home/u".to_string()),
            Some(PathBuf::from("/home/u/.config")),
            PathBuf::from("/proj"),
        )
        .unwrap();
        assert_eq!(e.home, PathBuf::from("/home/u"));
        assert_eq!(e.config_home, PathBuf::from("/home/u/.config"));
        assert_eq!(e.project_root, PathBuf::from("/proj"));
    }

    #[test]
    fn from_parts_errs_without_home() {
        assert!(
            AdapterEnv::from_parts(None, Some(PathBuf::from("/c")), PathBuf::from("/p")).is_err()
        );
    }

    #[test]
    fn from_parts_errs_without_config_home() {
        assert!(AdapterEnv::from_parts(Some("/h".to_string()), None, PathBuf::from("/p")).is_err());
    }

    #[test]
    fn adapters_dir_joins_under_config_home() {
        let e = AdapterEnv {
            home: PathBuf::from("/h"),
            config_home: PathBuf::from("/h/.config"),
            project_root: PathBuf::from("/p"),
        };
        assert_eq!(
            adapters_dir(&e),
            PathBuf::from("/h/.config/hector/adapters")
        );
    }
}
