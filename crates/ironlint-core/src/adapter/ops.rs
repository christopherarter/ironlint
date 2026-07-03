use crate::adapter::materialize::{
    atomic_write, backup_once, read_sidecar, sha256_hex, write_sidecar, AdapterSidecar,
};
use crate::adapter::plan::PlanStep;
use crate::adapter::registry::{JsonHookSpec, PluginSpec, SkillSpec};
use crate::adapter::SKILL_NAME;
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

pub fn install(h: &Harness, env: &AdapterEnv, scope: Scope) -> Result<InstallOutcome> {
    let result = match &h.kind {
        HarnessKind::JsonHook(spec) => install_jsonhook(h.name, spec, env, scope)?,
        HarnessKind::Plugin(spec) => install_plugin(spec, env, scope)?,
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
) -> Result<InstallResult> {
    let dir = adapters_dir(env).join(name);
    let primary_path = dir.join(spec.primary);
    let command = format!("\"{}\" {}", primary_path.display(), spec.entry_arg);
    let marker = format!("{}", dir.display());
    let settings = settings_path(spec, env, scope);

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
    Ok(match patch {
        PatchResult::AlreadyPresent => InstallResult::AlreadyPresent,
        PatchResult::Added => {
            write_settings(&settings, &value)?;
            InstallResult::Installed
        }
    })
}

fn install_plugin(spec: &PluginSpec, env: &AdapterEnv, scope: Scope) -> Result<InstallResult> {
    let dir = plugin_dir(spec, env, scope);
    let file = dir.join(spec.filename);
    let new_bytes = spec.source.as_bytes();
    let existed = file.exists();
    if existed {
        if let Ok(cur) = std::fs::read(&file) {
            if cur == new_bytes {
                return Ok(InstallResult::AlreadyPresent);
            }
        }
    }
    atomic_write(&file, new_bytes)?;
    let mut files = BTreeMap::new();
    files.insert(spec.filename.to_string(), sha256_hex(new_bytes));
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

fn skill_base(spec: &SkillSpec, env: &AdapterEnv, scope: Scope) -> PathBuf {
    match scope {
        Scope::Local => (spec.dir_local)(env),
        Scope::Global => (spec.dir_global)(env),
    }
}

pub fn install_skill(h: &Harness, env: &AdapterEnv, scope: Scope) -> Result<InstallOutcome> {
    let dir = skill_base(&h.skill, env, scope).join(SKILL_NAME);
    let file = dir.join("SKILL.md");
    let result = install_skill_file(&file, &dir, h.skill.source.as_bytes())?;
    Ok(InstallOutcome {
        harness: h.name,
        result,
        hint: h.restart_hint,
    })
}

fn install_skill_file(file: &Path, dir: &Path, bytes: &[u8]) -> Result<InstallResult> {
    let existed = file.exists();
    if existed {
        if let Ok(cur) = std::fs::read(file) {
            if cur == bytes {
                return Ok(InstallResult::AlreadyPresent);
            }
        }
    }
    atomic_write(file, bytes)?;
    let mut files = BTreeMap::new();
    files.insert("SKILL.md".to_string(), sha256_hex(bytes));
    write_sidecar(
        dir,
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

pub fn uninstall_skill(h: &Harness, env: &AdapterEnv, scope: Scope) -> Result<InstallOutcome> {
    let dir = skill_base(&h.skill, env, scope).join(SKILL_NAME);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(InstallOutcome {
        harness: h.name,
        result: InstallResult::Installed,
        hint: h.restart_hint,
    })
}

// --- plan (preview; writes nothing) ------------------------------------------

/// Full onboarding footprint for `h`: hook/plugin file(s), the settings patch
/// (JsonHook only), and the authoring skill. Computes paths only — no I/O.
pub fn plan_install(h: &Harness, env: &AdapterEnv, scope: Scope) -> Vec<PlanStep> {
    let mut steps = match &h.kind {
        HarnessKind::JsonHook(spec) => {
            let dir = adapters_dir(env).join(h.name);
            let mut v: Vec<PlanStep> = spec
                .files
                .iter()
                .map(|(f, _)| PlanStep::Hook { path: dir.join(f) })
                .collect();
            v.push(PlanStep::Patch {
                path: settings_path(spec, env, scope),
                key: spec.array_key,
            });
            v
        }
        HarnessKind::Plugin(spec) => vec![PlanStep::Plugin {
            path: plugin_dir(spec, env, scope).join(spec.filename),
        }],
    };
    let skill_dir = skill_base(&h.skill, env, scope).join(SKILL_NAME);
    steps.push(PlanStep::Skill {
        path: skill_dir.join("SKILL.md"),
    });
    steps
}

/// Removal footprint for `h`: the adapter dir (JsonHook) or plugin file
/// (Plugin), the settings patch (JsonHook), and the skill directory.
pub fn plan_uninstall(h: &Harness, env: &AdapterEnv, scope: Scope) -> Vec<PlanStep> {
    let mut steps = match &h.kind {
        HarnessKind::JsonHook(spec) => vec![
            PlanStep::Hook {
                path: adapters_dir(env).join(h.name),
            },
            PlanStep::Patch {
                path: settings_path(spec, env, scope),
                key: spec.array_key,
            },
        ],
        HarnessKind::Plugin(spec) => vec![PlanStep::Plugin {
            path: plugin_dir(spec, env, scope).join(spec.filename),
        }],
    };
    steps.push(PlanStep::Skill {
        path: skill_base(&h.skill, env, scope).join(SKILL_NAME),
    });
    steps
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

pub fn uninstall(h: &Harness, env: &AdapterEnv, scope: Scope) -> Result<InstallOutcome> {
    let result = match &h.kind {
        HarnessKind::JsonHook(spec) => uninstall_jsonhook(h.name, spec, env, scope)?,
        HarnessKind::Plugin(spec) => uninstall_plugin(h.name, spec, env, scope)?,
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
) -> Result<InstallResult> {
    let dir = adapters_dir(env).join(name);
    let settings = settings_path(spec, env, scope);
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
) -> Result<InstallResult> {
    let dir = plugin_dir(spec, env, scope);
    let file = dir.join(spec.filename);
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
    let installed = dir.exists();
    let (intact, current) = sidecar_integrity(&dir)?;
    Ok((installed, registered, intact, current))
}

fn status_plugin(
    spec: &PluginSpec,
    env: &AdapterEnv,
    scope: Scope,
) -> Result<(bool, bool, Option<bool>, Option<bool>)> {
    let dir = plugin_dir(spec, env, scope);
    let file = dir.join(spec.filename);
    let installed = file.exists();
    let registered = installed;
    let (intact, current) = sidecar_integrity(&dir)?;
    Ok((installed, registered, intact, current))
}

/// Compare every file recorded in the sidecar against the on-disk bytes in
/// `dir`. `intact = Some(true)` only when every recorded file is present and
/// its on-disk sha256 matches the sidecar's recorded hash. A missing file or
/// a differing hash yields `Some(false)`. No sidecar → `(None, None)`.
fn sidecar_integrity(dir: &Path) -> Result<(Option<bool>, Option<bool>)> {
    match read_sidecar(dir)? {
        Some(sc) => {
            let intact =
                sc.files.iter().all(
                    |(name, recorded_hash)| match std::fs::read(dir.join(name)) {
                        Ok(bytes) => sha256_hex(&bytes) == *recorded_hash,
                        Err(_) => false,
                    },
                );
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
    use crate::adapter::{
        all_harnesses, plan_install, plan_uninstall, AdapterEnv, PlanStep, Scope,
    };

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
    fn install_codex_writes_artifact_sidecar_and_patches_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let out = install(&harness("codex"), &e, Scope::Global).unwrap();
        assert!(matches!(out.result, InstallResult::Installed));
        let hook = e.config_home.join("ironlint/adapters/codex/hook.sh");
        assert!(hook.exists());
        assert!(crate::adapter::read_sidecar(hook.parent().unwrap())
            .unwrap()
            .is_some());
        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.path().join(".codex/hooks.json")).unwrap(),
        )
        .unwrap();
        let cmd = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(cmd.contains("adapters/codex/hook.sh"));
        assert!(cmd.ends_with("pre-tool-use"));
    }

    #[test]
    fn install_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        install(&harness("codex"), &e, Scope::Global).unwrap();
        let again = install(&harness("codex"), &e, Scope::Global).unwrap();
        assert!(matches!(again.result, InstallResult::AlreadyPresent));
    }

    #[test]
    fn install_plugin_drops_file_in_project_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let out = install(&harness("opencode"), &e, Scope::Local).unwrap();
        assert!(matches!(out.result, InstallResult::Installed));
        assert!(e
            .project_root
            .join(".opencode/plugins/ironlint.ts")
            .exists());
    }

    #[test]
    fn uninstall_removes_artifact_and_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        install(&harness("codex"), &e, Scope::Global).unwrap();
        let out = uninstall(&harness("codex"), &e, Scope::Global).unwrap();
        assert!(matches!(out.result, InstallResult::Installed)); // "removed" reuses Installed-style ok
        assert!(!e
            .config_home
            .join("ironlint/adapters/codex/hook.sh")
            .exists());
        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.path().join(".codex/hooks.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn status_reports_installed_and_intact_after_install() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        std::fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        install(&harness("codex"), &e, Scope::Global).unwrap();
        let st = status(&harness("codex"), &e, Scope::Global).unwrap();
        assert!(st.detected && st.installed && st.registered);
        assert_eq!(st.intact, Some(true));
        assert_eq!(st.current, Some(true));
    }

    #[test]
    fn install_jsonhook_idempotent_leaves_settings_byte_identical() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        install(&harness("codex"), &e, Scope::Global).unwrap();
        let settings_path = tmp.path().join(".codex/hooks.json");
        let before = std::fs::read_to_string(&settings_path).unwrap();
        install(&harness("codex"), &e, Scope::Global).unwrap();
        let after = std::fs::read_to_string(&settings_path).unwrap();
        assert_eq!(
            before, after,
            "settings file must not be rewritten on AlreadyPresent"
        );
    }

    #[test]
    fn install_plugin_identical_content_is_already_present() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        install(&harness("opencode"), &e, Scope::Local).unwrap();
        let again = install(&harness("opencode"), &e, Scope::Local).unwrap();
        assert!(
            matches!(again.result, InstallResult::AlreadyPresent),
            "identical re-install must return AlreadyPresent"
        );
    }

    #[test]
    fn install_plugin_changed_content_is_updated() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        install(&harness("opencode"), &e, Scope::Local).unwrap();
        let file = e.project_root.join(".opencode/plugins/ironlint.ts");
        std::fs::write(&file, b"// changed").unwrap();
        let again = install(&harness("opencode"), &e, Scope::Local).unwrap();
        assert!(
            matches!(again.result, InstallResult::Updated),
            "changed content must return Updated"
        );
    }

    #[test]
    fn status_before_install_is_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let st = status(&harness("codex"), &e, Scope::Global).unwrap();
        assert!(!st.installed);
        assert!(!st.registered);
        assert!(st.intact.is_none());
        assert!(st.current.is_none());
    }

    #[test]
    fn status_detects_on_disk_tamper() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let h = harness("codex");
        install(&h, &e, Scope::Global).unwrap();
        // Tamper with the installed artifact, leaving the sidecar untouched.
        let hook = e.config_home.join("ironlint/adapters/codex/hook.sh");
        let mut bytes = std::fs::read(&hook).unwrap();
        bytes.extend_from_slice(b"\n# tampered\n");
        std::fs::write(&hook, bytes).unwrap();
        let st = status(&h, &e, Scope::Global).unwrap();
        assert_eq!(
            st.intact,
            Some(false),
            "edited on-disk artifact must report intact=false"
        );
    }

    #[test]
    fn uninstall_plugin_removes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        install(&harness("opencode"), &e, Scope::Local).unwrap();
        let file = e.project_root.join(".opencode/plugins/ironlint.ts");
        assert!(file.exists());
        uninstall(&harness("opencode"), &e, Scope::Local).unwrap();
        assert!(!file.exists(), "uninstall must remove the plugin file");
    }

    #[test]
    fn install_skill_writes_skill_md_and_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let out = install_skill(&harness("pi"), &e, Scope::Local).unwrap();
        assert!(matches!(out.result, InstallResult::Installed));
        let skill = e.project_root.join(".pi/skills/ironlint-config/SKILL.md");
        assert!(skill.exists(), "SKILL.md must land at {}", skill.display());
        assert!(crate::adapter::read_sidecar(skill.parent().unwrap())
            .unwrap()
            .is_some());
    }

    #[test]
    fn install_skill_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        install_skill(&harness("pi"), &e, Scope::Local).unwrap();
        let again = install_skill(&harness("pi"), &e, Scope::Local).unwrap();
        assert!(matches!(again.result, InstallResult::AlreadyPresent));
    }

    #[test]
    fn install_skill_changed_content_is_updated() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        install_skill(&harness("pi"), &e, Scope::Local).unwrap();
        let f = e.project_root.join(".pi/skills/ironlint-config/SKILL.md");
        std::fs::write(&f, b"// tampered").unwrap();
        let again = install_skill(&harness("pi"), &e, Scope::Local).unwrap();
        assert!(matches!(again.result, InstallResult::Updated));
    }

    #[test]
    fn uninstall_skill_removes_the_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        install_skill(&harness("pi"), &e, Scope::Local).unwrap();
        let dir = e.project_root.join(".pi/skills/ironlint-config");
        assert!(dir.exists());
        uninstall_skill(&harness("pi"), &e, Scope::Local).unwrap();
        assert!(!dir.exists(), "uninstall must remove the skill dir");
    }

    #[test]
    fn install_skill_global_uses_home_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        install_skill(&harness("pi"), &e, Scope::Global).unwrap();
        // pi global skills dir is ~/.pi/agent/skills
        assert!(e
            .home
            .join(".pi/agent/skills/ironlint-config/SKILL.md")
            .exists());
    }

    #[test]
    fn plan_install_jsonhook_lists_files_patch_and_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let steps = plan_install(&harness("claude-code"), &e, Scope::Local);
        // one hook file + one patch + one skill
        let hooks = steps
            .iter()
            .filter(|s| matches!(s, PlanStep::Hook { .. }))
            .count();
        assert_eq!(hooks, 1, "claude-code ships hook.sh");
        assert!(steps
            .iter()
            .any(|s| matches!(s, PlanStep::Patch { key, .. } if *key == "PreToolUse")));
        assert!(steps.iter().any(|s| matches!(s, PlanStep::Skill { .. })));
    }

    #[test]
    fn plan_install_plugin_lists_plugin_and_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let steps = plan_install(&harness("pi"), &e, Scope::Local);
        assert!(steps.iter().any(|s| matches!(s, PlanStep::Plugin { .. })));
        assert!(steps.iter().any(|s| matches!(s, PlanStep::Skill { .. })));
        assert!(!steps.iter().any(|s| matches!(s, PlanStep::Patch { .. })));
    }

    #[test]
    fn plan_install_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let _ = plan_install(&harness("codex"), &e, Scope::Global);
        assert!(!e
            .config_home
            .join("ironlint/adapters/codex/hook.sh")
            .exists());
        assert!(!tmp.path().join(".codex/hooks.json").exists());
    }

    #[test]
    fn plan_uninstall_jsonhook_lists_dir_patch_and_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let steps = plan_uninstall(&harness("codex"), &e, Scope::Global);
        assert!(steps.iter().any(|s| matches!(s, PlanStep::Hook { .. })));
        assert!(steps.iter().any(|s| matches!(s, PlanStep::Patch { .. })));
        assert!(steps.iter().any(|s| matches!(s, PlanStep::Skill { .. })));
    }

    #[test]
    fn plan_uninstall_plugin_lists_file_and_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let steps = plan_uninstall(&harness("opencode"), &e, Scope::Local);
        assert!(steps.iter().any(|s| matches!(s, PlanStep::Plugin { .. })));
        assert!(steps.iter().any(|s| matches!(s, PlanStep::Skill { .. })));
    }
}
