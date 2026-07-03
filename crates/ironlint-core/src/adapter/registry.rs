use crate::adapter::{AdapterEnv, Harness, HarnessKind};
use serde_json::{json, Value};
use std::path::PathBuf;

// --- embedded artifacts (single source of truth = adapters/) -----------------
const CLAUDE_HOOK: &str = include_str!("../../../../adapters/claude-code/hooks/hook.sh");
const REASONIX_HOOK: &str = include_str!("../../../../adapters/reasonix/hooks/hook.sh");
const PI_PLUGIN: &str = include_str!("../../../../adapters/pi/src/index.ts");
const OPENCODE_PLUGIN: &str = include_str!("../../../../adapters/opencode/src/index.ts");
const IRONLINT_CONFIG_SKILL: &str =
    include_str!("../../../../adapters/shared/ironlint-config/SKILL.md");

/// Skill name and install-dir leaf for the authoring skill.
pub const SKILL_NAME: &str = "ironlint-config";

#[derive(Clone, Copy)]
pub struct JsonHookSpec {
    pub settings_local: fn(&AdapterEnv) -> Option<PathBuf>,
    pub settings_global: fn(&AdapterEnv) -> PathBuf,
    pub array_key: &'static str,
    pub entry_arg: &'static str,
    pub primary: &'static str,
    pub files: &'static [(&'static str, &'static str)],
    pub build_entry: fn(&str) -> Value,
}

#[derive(Clone, Copy)]
pub struct PluginSpec {
    pub dir_local: fn(&AdapterEnv) -> Option<PathBuf>,
    pub dir_global: fn(&AdapterEnv) -> Option<PathBuf>,
    pub filename: &'static str,
    pub source: &'static str,
    pub detect: fn(&AdapterEnv) -> bool,
}

/// Where a harness discovers `SKILL.md` files, and the shared skill bytes.
#[derive(Clone, Copy)]
pub struct SkillSpec {
    pub dir_local: fn(&AdapterEnv) -> PathBuf,
    pub dir_global: fn(&AdapterEnv) -> PathBuf,
    pub source: &'static str,
}

// --- per-harness entry builders (also unit-tested directly) ------------------
pub(crate) fn claude_build_entry(command: &str) -> Value {
    json!({"matcher": "Edit|Write",
           "hooks": [{"type": "command", "command": command}]})
}

pub(crate) fn reasonix_build_entry(command: &str) -> Value {
    json!({"command": command,
           "match": "^(write_file|edit_file|multi_edit)$",
           "description": "Block edits that violate ironlint policy before they land on disk",
           "timeout": 30000})
}

// --- registry ----------------------------------------------------------------
const CLAUDE: JsonHookSpec = JsonHookSpec {
    settings_local: |e| Some(e.project_root.join(".claude").join("settings.json")),
    settings_global: |e| e.home.join(".claude").join("settings.json"),
    array_key: "PreToolUse",
    entry_arg: "pre-tool-use",
    primary: "hook.sh",
    files: &[("hook.sh", CLAUDE_HOOK)],
    build_entry: claude_build_entry,
};

const REASONIX: JsonHookSpec = JsonHookSpec {
    settings_local: |_| None, // reasonix settings are user-global only
    settings_global: |e| e.home.join(".reasonix").join("settings.json"),
    array_key: "PreToolUse",
    entry_arg: "pre-tool-use",
    primary: "hook.sh",
    files: &[("hook.sh", REASONIX_HOOK)],
    build_entry: reasonix_build_entry,
};

const PI: PluginSpec = PluginSpec {
    dir_local: |e| Some(e.project_root.join(".pi").join("extensions")),
    dir_global: |e| Some(e.home.join(".pi").join("agent").join("extensions")),
    filename: "ironlint.ts",
    source: PI_PLUGIN,
    detect: |e| e.home.join(".pi").is_dir(),
};

const OPENCODE: PluginSpec = PluginSpec {
    dir_local: |e| Some(e.project_root.join(".opencode").join("plugins")),
    dir_global: |_| None, // opencode plugins are project-scoped (per adapter README)
    filename: "ironlint.ts",
    source: OPENCODE_PLUGIN,
    detect: |e| {
        e.config_home.join("opencode").is_dir() || e.project_root.join(".opencode").is_dir()
    },
};

// --- per-harness skill specs -------------------------------------------------
const CLAUDE_SKILL: SkillSpec = SkillSpec {
    dir_local: |e| e.project_root.join(".claude").join("skills"),
    dir_global: |e| e.home.join(".claude").join("skills"),
    source: IRONLINT_CONFIG_SKILL,
};
const REASONIX_SKILL: SkillSpec = SkillSpec {
    dir_local: |e| e.project_root.join(".reasonix").join("skills"),
    dir_global: |e| e.home.join(".reasonix").join("skills"),
    source: IRONLINT_CONFIG_SKILL,
};
const PI_SKILL: SkillSpec = SkillSpec {
    dir_local: |e| e.project_root.join(".pi").join("skills"),
    dir_global: |e| e.home.join(".pi").join("agent").join("skills"),
    source: IRONLINT_CONFIG_SKILL,
};
const OPENCODE_SKILL: SkillSpec = SkillSpec {
    dir_local: |e| e.project_root.join(".opencode").join("skills"),
    dir_global: |e| e.config_home.join("opencode").join("skills"),
    source: IRONLINT_CONFIG_SKILL,
};

pub fn all_harnesses() -> Vec<Harness> {
    vec![
        Harness {
            name: "claude-code",
            kind: HarnessKind::JsonHook(CLAUDE),
            restart_hint: "Reload Claude Code (or restart) — it picks up settings.json hooks.",
            skill: CLAUDE_SKILL,
        },
        Harness {
            name: "reasonix",
            kind: HarnessKind::JsonHook(REASONIX),
            restart_hint: "Restart Reasonix so it reloads settings.",
            skill: REASONIX_SKILL,
        },
        Harness {
            name: "pi",
            kind: HarnessKind::Plugin(PI),
            restart_hint: "Restart pi so it loads the new extension.",
            skill: PI_SKILL,
        },
        Harness {
            name: "opencode",
            kind: HarnessKind::Plugin(OPENCODE),
            restart_hint: "Restart opencode so it loads the new plugin.",
            skill: OPENCODE_SKILL,
        },
    ]
}

/// Whether `harness` looks installed on this machine.
///
/// Keyed on the harness **name**, not `array_key`: claude-code and reasonix
/// both register a `PreToolUse` hook (see `CLAUDE`/`REASONIX` above), so a
/// dispatch on `array_key` alone can no longer distinguish them.
pub(crate) fn is_detected(harness: &Harness, env: &AdapterEnv) -> bool {
    match &harness.kind {
        HarnessKind::JsonHook(_) => match harness.name {
            "claude-code" => env.home.join(".claude").is_dir(),
            "reasonix" => env.home.join(".reasonix").is_dir(),
            _ => false,
        },
        HarnessKind::Plugin(p) => (p.detect)(env),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{AdapterEnv, HarnessKind};
    use std::path::PathBuf;

    fn env_with(home: &str, project: &str) -> AdapterEnv {
        AdapterEnv {
            home: PathBuf::from(home),
            config_home: PathBuf::from(format!("{home}/.config")),
            project_root: PathBuf::from(project),
        }
    }

    #[test]
    fn four_harnesses_registered() {
        let names: Vec<_> = all_harnesses().iter().map(|h| h.name).collect();
        assert_eq!(names, vec!["claude-code", "reasonix", "pi", "opencode"]);
    }

    #[test]
    fn embedded_artifacts_are_nonempty() {
        for h in all_harnesses() {
            match &h.kind {
                HarnessKind::JsonHook(s) => {
                    assert!(!s.files.is_empty(), "{} has no files", h.name);
                    for (name, bytes) in s.files {
                        assert!(!bytes.is_empty(), "{}/{} empty", h.name, name);
                    }
                }
                HarnessKind::Plugin(p) => assert!(!p.source.is_empty(), "{} plugin empty", h.name),
            }
        }
    }

    #[test]
    fn claude_entry_points_at_command_and_matcher() {
        let e = claude_build_entry("\"/x/hook.sh\" pre-tool-use");
        assert_eq!(e["matcher"], "Edit|Write");
        assert_eq!(e["hooks"][0]["command"], "\"/x/hook.sh\" pre-tool-use");
    }

    #[test]
    fn reasonix_entry_matches_write_tools() {
        let e = reasonix_build_entry("\"/x/hook.sh\" pre-tool-use");
        assert_eq!(e["match"], "^(write_file|edit_file|multi_edit)$");
        assert_eq!(e["command"], "\"/x/hook.sh\" pre-tool-use");
    }

    #[test]
    fn detect_reports_presence_per_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_str().unwrap();
        std::fs::create_dir_all(format!("{home}/.claude")).unwrap();
        std::fs::create_dir_all(format!("{home}/.pi")).unwrap();
        let env = env_with(home, home);
        let found: std::collections::BTreeMap<_, _> =
            crate::adapter::detect(&env).into_iter().collect();
        assert!(found["claude-code"]);
        assert!(found["pi"]);
        assert!(!found["reasonix"]);
        assert!(!found["opencode"]);
    }

    #[test]
    fn claude_and_reasonix_share_pre_tool_use_but_detect_independently() {
        // Regression guard: claude-code and reasonix both register a
        // PreToolUse hook now, so `is_detected` must key off the harness
        // name, not `array_key` — otherwise claude-code would be (mis)detected
        // via ~/.reasonix, or vice versa.
        let harnesses = all_harnesses();
        let claude = harnesses.iter().find(|h| h.name == "claude-code").unwrap();
        let reasonix = harnesses.iter().find(|h| h.name == "reasonix").unwrap();
        match (&claude.kind, &reasonix.kind) {
            (HarnessKind::JsonHook(c), HarnessKind::JsonHook(r)) => {
                assert_eq!(c.array_key, "PreToolUse");
                assert_eq!(r.array_key, "PreToolUse");
            }
            _ => panic!("expected both to be JsonHook"),
        }

        // Only ~/.reasonix exists: claude-code must NOT be detected.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_str().unwrap();
        std::fs::create_dir_all(format!("{home}/.reasonix")).unwrap();
        let env = env_with(home, home);
        assert!(
            !is_detected(claude, &env),
            "claude-code false-positive via ~/.reasonix"
        );
        assert!(is_detected(reasonix, &env));

        // Only ~/.claude exists: reasonix must NOT be detected.
        let tmp2 = tempfile::tempdir().unwrap();
        let home2 = tmp2.path().to_str().unwrap();
        std::fs::create_dir_all(format!("{home2}/.claude")).unwrap();
        let env2 = env_with(home2, home2);
        assert!(is_detected(claude, &env2));
        assert!(
            !is_detected(reasonix, &env2),
            "reasonix false-positive via ~/.claude"
        );
    }

    #[test]
    fn skill_dirs_resolve_per_harness() {
        let env = env_with("/home/u", "/home/u/proj");
        let by = |name: &str| {
            all_harnesses()
                .into_iter()
                .find(|harness| harness.name == name)
                .unwrap()
                .skill
        };
        // claude-code
        let claude = by("claude-code");
        assert_eq!(
            (claude.dir_local)(&env),
            PathBuf::from("/home/u/proj/.claude/skills")
        );
        assert_eq!(
            (claude.dir_global)(&env),
            PathBuf::from("/home/u/.claude/skills")
        );
        // pi
        let pi = by("pi");
        assert_eq!(
            (pi.dir_local)(&env),
            PathBuf::from("/home/u/proj/.pi/skills")
        );
        assert_eq!(
            (pi.dir_global)(&env),
            PathBuf::from("/home/u/.pi/agent/skills")
        );
        // opencode (global lives under config_home)
        let opencode = by("opencode");
        assert_eq!(
            (opencode.dir_local)(&env),
            PathBuf::from("/home/u/proj/.opencode/skills")
        );
        assert_eq!(
            (opencode.dir_global)(&env),
            PathBuf::from("/home/u/.config/opencode/skills")
        );
        // reasonix
        let reasonix = by("reasonix");
        assert_eq!(
            (reasonix.dir_local)(&env),
            PathBuf::from("/home/u/proj/.reasonix/skills")
        );
        assert_eq!(
            (reasonix.dir_global)(&env),
            PathBuf::from("/home/u/.reasonix/skills")
        );
    }

    #[test]
    fn every_harness_ships_the_same_skill_source() {
        for h in all_harnesses() {
            assert!(
                h.skill.source.contains("name: ironlint-config"),
                "{} skill source wrong",
                h.name
            );
        }
    }

    #[test]
    fn embedded_set_covers_on_disk_adapter_files() {
        // Drift guard: every shell/ts file shipped under adapters/<h> for a
        // hook-capable harness must be embedded, else `ironlint init` ships a
        // partial hook. Checks the two JsonHook harnesses' hooks/ dirs.
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../adapters");
        for (harness, subdir) in [("claude-code", "hooks"), ("reasonix", "hooks")] {
            let dir = root.join(harness).join(subdir);
            let spec = match &all_harnesses()
                .into_iter()
                .find(|h| h.name == harness)
                .unwrap()
                .kind
            {
                HarnessKind::JsonHook(s) => *s,
                _ => unreachable!(),
            };
            for entry in std::fs::read_dir(&dir).unwrap() {
                let name = entry.unwrap().file_name().into_string().unwrap();
                if std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("sh"))
                {
                    assert!(
                        spec.files.iter().any(|(f, _)| *f == name),
                        "adapters/{harness}/{subdir}/{name} is not embedded in the registry"
                    );
                }
            }
        }
    }
}
