use crate::adapter::materialize::{
    atomic_write, backup_once, read_sidecar, sha256_hex, write_sidecar, AdapterSidecar,
};
use crate::adapter::registry::{JsonHookSpec, PluginSpec};
use crate::adapter::{
    adapters_dir, remove_from_hook_array, sync_hook_array, AdapterEnv, Harness, HarnessKind,
    PatchResult, Scope, CURRENT_ADAPTER_VERSION,
};
use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum InstallResult {
    Installed,
    AlreadyPresent,
    Updated,
    Skipped(String),
    Failed(String),
    DryRun(Vec<String>),
}

pub struct InstallOutcome {
    pub harness: &'static str,
    pub result: InstallResult,
    pub hint: &'static str,
}

pub struct HarnessStatus {
    pub harness: &'static str,
    pub detected: bool,
    pub installed: bool,
    pub registered: bool,
    pub intact: Option<bool>,
    pub current: Option<bool>,
}

/// Read a JSON settings file, defaulting to `{}` when absent.
fn load_settings(path: &Path) -> Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::default())),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_settings(path: &Path, value: &Value) -> Result<()> {
    backup_once(path)?;
    let json = serde_json::to_string_pretty(value)?;
    atomic_write(path, json.as_bytes())
}

fn settings_path(spec: &JsonHookSpec, env: &AdapterEnv, scope: Scope) -> PathBuf {
    match scope {
        Scope::Local => (spec.settings_local)(env).unwrap_or_else(|| (spec.settings_global)(env)),
        Scope::Global => (spec.settings_global)(env),
    }
}

fn plugin_dir(spec: &PluginSpec, env: &AdapterEnv, scope: Scope) -> PathBuf {
    let (primary, fallback) = match scope {
        Scope::Local => ((spec.dir_local)(env), (spec.dir_global)(env)),
        Scope::Global => ((spec.dir_global)(env), (spec.dir_local)(env)),
    };
    primary
        .or(fallback)
        .expect("every plugin harness has at least one dir")
}

// --- install -----------------------------------------------------------------

pub fn install(
    h: &Harness,
    env: &AdapterEnv,
    scope: Scope,
    dry_run: bool,
) -> Result<InstallOutcome> {
    let result = match &h.kind {
        HarnessKind::JsonHook(spec) => install_jsonhook(h.name, spec, env, scope, dry_run)?,
        HarnessKind::Plugin(spec) => install_plugin(spec, env, scope, dry_run)?,
    };
    Ok(InstallOutcome {
        harness: h.name,
        result,
        hint: h.restart_hint,
    })
}

fn install_jsonhook(
    name: &str,
    spec: &JsonHookSpec,
    env: &AdapterEnv,
    scope: Scope,
    dry_run: bool,
) -> Result<InstallResult> {
    let dir = adapters_dir(env).join(name);
    let primary_path = dir.join(spec.primary);
    let command = format!("\"{}\" {}", primary_path.display(), spec.entry_arg);
    let marker = format!("{}", dir.display());
    let settings = settings_path(spec, env, scope);

    if dry_run {
        let mut plan: Vec<String> = spec
            .files
            .iter()
            .map(|(f, _)| format!("write {}", dir.join(f).display()))
            .collect();
        plan.push(format!("patch {} [{}]", settings.display(), spec.array_key));
        return Ok(InstallResult::DryRun(plan));
    }

    let mut files = BTreeMap::new();
    for (fname, bytes) in spec.files {
        let p = dir.join(fname);
        atomic_write(&p, bytes.as_bytes())?;
        set_executable(&p)?;
        files.insert((*fname).to_string(), sha256_hex(bytes.as_bytes()));
    }
    write_sidecar(
        &dir,
        &AdapterSidecar {
            version: CURRENT_ADAPTER_VERSION,
            files,
        },
    )?;

    let mut value = load_settings(&settings)?;
    let entry = (spec.build_entry)(&command);
    let patch = sync_hook_array(&mut value, spec.array_key, entry, &marker);
    write_settings(&settings, &value)?;
    Ok(match patch {
        PatchResult::AlreadyPresent => InstallResult::AlreadyPresent,
        PatchResult::Added => InstallResult::Installed,
    })
}

fn install_plugin(
    spec: &PluginSpec,
    env: &AdapterEnv,
    scope: Scope,
    dry_run: bool,
) -> Result<InstallResult> {
    let dir = plugin_dir(spec, env, scope);
    let file = dir.join(spec.filename);
    if dry_run {
        return Ok(InstallResult::DryRun(vec![format!(
            "write {}",
            file.display()
        )]));
    }
    let existed = file.exists();
    atomic_write(&file, spec.source.as_bytes())?;
    let mut files = BTreeMap::new();
    files.insert(
        spec.filename.to_string(),
        sha256_hex(spec.source.as_bytes()),
    );
    write_sidecar(
        &dir,
        &AdapterSidecar {
            version: CURRENT_ADAPTER_VERSION,
            files,
        },
    )?;
    Ok(if existed {
        InstallResult::Updated
    } else {
        InstallResult::Installed
    })
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}
#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

// --- uninstall ---------------------------------------------------------------

pub fn uninstall(
    h: &Harness,
    env: &AdapterEnv,
    scope: Scope,
    dry_run: bool,
) -> Result<InstallOutcome> {
    let result = match &h.kind {
        HarnessKind::JsonHook(spec) => uninstall_jsonhook(h.name, spec, env, scope, dry_run)?,
        HarnessKind::Plugin(spec) => uninstall_plugin(h.name, spec, env, scope, dry_run)?,
    };
    Ok(InstallOutcome {
        harness: h.name,
        result,
        hint: h.restart_hint,
    })
}

fn uninstall_jsonhook(
    name: &str,
    spec: &JsonHookSpec,
    env: &AdapterEnv,
    scope: Scope,
    dry_run: bool,
) -> Result<InstallResult> {
    let dir = adapters_dir(env).join(name);
    let settings = settings_path(spec, env, scope);
    if dry_run {
        return Ok(InstallResult::DryRun(vec![
            format!("remove {}", dir.display()),
            format!("unpatch {} [{}]", settings.display(), spec.array_key),
        ]));
    }
    if settings.exists() {
        let mut value = load_settings(&settings)?;
        if remove_from_hook_array(&mut value, spec.array_key, &format!("{}", dir.display())) {
            write_settings(&settings, &value)?;
        }
    }
    remove_dir_if_present(&dir)?;
    Ok(InstallResult::Installed)
}

fn uninstall_plugin(
    _name: &str,
    spec: &PluginSpec,
    env: &AdapterEnv,
    scope: Scope,
    dry_run: bool,
) -> Result<InstallResult> {
    let dir = plugin_dir(spec, env, scope);
    let file = dir.join(spec.filename);
    if dry_run {
        return Ok(InstallResult::DryRun(vec![format!(
            "remove {}",
            file.display()
        )]));
    }
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_file(crate::adapter::sidecar_path(&dir));
    Ok(InstallResult::Installed)
}

fn remove_dir_if_present(dir: &Path) -> Result<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", dir.display())),
    }
}

// --- status ------------------------------------------------------------------

pub fn status(h: &Harness, env: &AdapterEnv, scope: Scope) -> Result<HarnessStatus> {
    let detected = crate::adapter::registry::is_detected(h, env);
    let (installed, registered, intact, current) = match &h.kind {
        HarnessKind::JsonHook(spec) => status_jsonhook(h.name, spec, env, scope, detected)?,
        HarnessKind::Plugin(spec) => status_plugin(spec, env, scope)?,
    };
    Ok(HarnessStatus {
        harness: h.name,
        detected,
        installed,
        registered,
        intact,
        current,
    })
}

fn status_jsonhook(
    name: &str,
    spec: &JsonHookSpec,
    env: &AdapterEnv,
    scope: Scope,
    _detected: bool,
) -> Result<(bool, bool, Option<bool>, Option<bool>)> {
    let dir = adapters_dir(env).join(name);
    let settings = settings_path(spec, env, scope);
    let registered = settings_has_marker(&settings, spec.array_key, &format!("{}", dir.display()))?;
    let expected: Vec<(String, String)> = spec
        .files
        .iter()
        .map(|(f, b)| ((*f).to_string(), sha256_hex(b.as_bytes())))
        .collect();
    let installed = dir.exists();
    let (intact, current) = sidecar_integrity(&dir, &expected)?;
    Ok((installed, registered, intact, current))
}

fn status_plugin(
    spec: &PluginSpec,
    env: &AdapterEnv,
    scope: Scope,
) -> Result<(bool, bool, Option<bool>, Option<bool>)> {
    let dir = plugin_dir(spec, env, scope);
    let file = dir.join(spec.filename);
    let expected = vec![(
        spec.filename.to_string(),
        sha256_hex(spec.source.as_bytes()),
    )];
    let installed = file.exists();
    let registered = installed;
    let (intact, current) = sidecar_integrity(&dir, &expected)?;
    Ok((installed, registered, intact, current))
}

fn sidecar_integrity(
    sidecar_dir: &Path,
    expected: &[(String, String)],
) -> Result<(Option<bool>, Option<bool>)> {
    match read_sidecar(sidecar_dir)? {
        Some(sc) => {
            let intact = expected
                .iter()
                .all(|(name, hash)| sc.files.get(name) == Some(hash));
            Ok((Some(intact), Some(sc.version == CURRENT_ADAPTER_VERSION)))
        }
        None => Ok((None, None)),
    }
}

fn settings_has_marker(path: &Path, key: &str, marker: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let value = load_settings(path)?;
    Ok(value
        .get("hooks")
        .and_then(|h| h.get(key))
        .map(|arr| {
            serde_json::to_string(arr)
                .unwrap_or_default()
                .contains(marker)
        })
        .unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{all_harnesses, AdapterEnv, Scope};

    fn harness(name: &str) -> crate::adapter::Harness {
        all_harnesses()
            .into_iter()
            .find(|h| h.name == name)
            .unwrap()
    }
    fn env(tmp: &std::path::Path) -> AdapterEnv {
        AdapterEnv {
            home: tmp.to_path_buf(),
            config_home: tmp.join(".config"),
            project_root: tmp.join("proj"),
        }
    }

    #[test]
    fn install_reasonix_writes_artifact_sidecar_and_patches_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let out = install(&harness("reasonix"), &e, Scope::Global, false).unwrap();
        assert!(matches!(out.result, InstallResult::Installed));
        let hook = e.config_home.join("hector/adapters/reasonix/hook.sh");
        assert!(hook.exists());
        assert!(crate::adapter::read_sidecar(hook.parent().unwrap())
            .unwrap()
            .is_some());
        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.path().join(".reasonix/settings.json")).unwrap(),
        )
        .unwrap();
        let cmd = settings["hooks"]["PreToolUse"][0]["command"]
            .as_str()
            .unwrap();
        assert!(cmd.contains("adapters/reasonix/hook.sh"));
        assert!(cmd.ends_with("pre-tool-use"));
    }

    #[test]
    fn install_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        install(&harness("reasonix"), &e, Scope::Global, false).unwrap();
        let again = install(&harness("reasonix"), &e, Scope::Global, false).unwrap();
        assert!(matches!(again.result, InstallResult::AlreadyPresent));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let out = install(&harness("reasonix"), &e, Scope::Global, true).unwrap();
        assert!(matches!(out.result, InstallResult::DryRun(_)));
        assert!(!tmp.path().join(".reasonix/settings.json").exists());
        assert!(!e
            .config_home
            .join("hector/adapters/reasonix/hook.sh")
            .exists());
    }

    #[test]
    fn install_plugin_drops_file_in_project_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let out = install(&harness("opencode"), &e, Scope::Local, false).unwrap();
        assert!(matches!(out.result, InstallResult::Installed));
        assert!(e.project_root.join(".opencode/plugins/hector.ts").exists());
    }

    #[test]
    fn uninstall_removes_artifact_and_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        install(&harness("reasonix"), &e, Scope::Global, false).unwrap();
        let out = uninstall(&harness("reasonix"), &e, Scope::Global, false).unwrap();
        assert!(matches!(out.result, InstallResult::Installed)); // "removed" reuses Installed-style ok
        assert!(!e
            .config_home
            .join("hector/adapters/reasonix/hook.sh")
            .exists());
        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.path().join(".reasonix/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn status_reports_installed_and_intact_after_install() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        std::fs::create_dir_all(tmp.path().join(".reasonix")).unwrap();
        install(&harness("reasonix"), &e, Scope::Global, false).unwrap();
        let st = status(&harness("reasonix"), &e, Scope::Global).unwrap();
        assert!(st.detected && st.installed && st.registered);
        assert_eq!(st.intact, Some(true));
        assert_eq!(st.current, Some(true));
    }
}
