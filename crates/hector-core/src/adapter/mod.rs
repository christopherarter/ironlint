//! Harness onboarding: materialize Hector's hook into supported coding agents.
mod json_settings;
mod materialize;
mod registry;

pub use json_settings::{remove_from_hook_array, sync_hook_array, PatchResult};
pub use materialize::{
    atomic_write, backup_once, read_sidecar, sha256_hex, sidecar_path, write_sidecar,
    AdapterSidecar,
};
pub use registry::{all_harnesses, JsonHookSpec, PluginSpec};

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
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .map_err(|_| anyhow::anyhow!("cannot resolve $HOME"))?;
        let config_home = crate::trust::config_home().ok_or_else(|| {
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
