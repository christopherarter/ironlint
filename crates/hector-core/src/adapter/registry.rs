use crate::adapter::{AdapterEnv, Harness, HarnessKind};
use serde_json::{json, Value};
use std::path::PathBuf;

// --- embedded artifacts (single source of truth = adapters/) -----------------
const CLAUDE_HOOK: &str = include_str!("../../../../adapters/claude-code/hooks/hook.sh");
const CLAUDE_SYNTH: &str =
    include_str!("../../../../adapters/claude-code/hooks/synthesize_diff.sh");
const REASONIX_HOOK: &str = include_str!("../../../../adapters/reasonix/hooks/hook.sh");
const PI_PLUGIN: &str = include_str!("../../../../adapters/pi/src/index.ts");
const OPENCODE_PLUGIN: &str = include_str!("../../../../adapters/opencode/src/index.ts");

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

// --- per-harness entry builders (also unit-tested directly) ------------------
pub(crate) fn claude_build_entry(command: &str) -> Value {
    json!({"matcher": "Edit|Write",
           "hooks": [{"type": "command", "command": command}]})
}

pub(crate) fn reasonix_build_entry(command: &str) -> Value {
    json!({"command": command,
           "match": "^(write_file|edit_file|multi_edit)$",
           "description": "Block edits that violate hector policy before they land on disk",
           "timeout": 30000})
}

// --- registry ----------------------------------------------------------------
const CLAUDE: JsonHookSpec = JsonHookSpec {
    settings_local: |e| Some(e.project_root.join(".claude").join("settings.json")),
    settings_global: |e| e.home.join(".claude").join("settings.json"),
    array_key: "PostToolUse",
    entry_arg: "post-tool-use",
    primary: "hook.sh",
    files: &[
        ("hook.sh", CLAUDE_HOOK),
        ("synthesize_diff.sh", CLAUDE_SYNTH),
    ],
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
    filename: "hector.ts",
    source: PI_PLUGIN,
    detect: |e| e.home.join(".pi").is_dir(),
};

const OPENCODE: PluginSpec = PluginSpec {
    dir_local: |e| Some(e.project_root.join(".opencode").join("plugins")),
    dir_global: |_| None, // opencode plugins are project-scoped (per adapter README)
    filename: "hector.ts",
    source: OPENCODE_PLUGIN,
    detect: |e| {
        e.config_home.join("opencode").is_dir() || e.project_root.join(".opencode").is_dir()
    },
};

pub fn all_harnesses() -> Vec<Harness> {
    vec![
        Harness {
            name: "claude-code",
            kind: HarnessKind::JsonHook(CLAUDE),
            restart_hint: "Reload Claude Code (or restart) — it picks up settings.json hooks.",
        },
        Harness {
            name: "reasonix",
            kind: HarnessKind::JsonHook(REASONIX),
            restart_hint: "Restart Reasonix so it reloads settings.",
        },
        Harness {
            name: "pi",
            kind: HarnessKind::Plugin(PI),
            restart_hint: "Restart pi so it loads the new extension.",
        },
        Harness {
            name: "opencode",
            kind: HarnessKind::Plugin(OPENCODE),
            restart_hint: "Restart opencode so it loads the new plugin.",
        },
    ]
}

/// Whether `harness` looks installed on this machine.
pub(crate) fn is_detected(harness: &Harness, env: &AdapterEnv) -> bool {
    match &harness.kind {
        HarnessKind::JsonHook(s) => match s.array_key {
            "PostToolUse" => env.home.join(".claude").is_dir(), // claude-code
            _ => env.home.join(".reasonix").is_dir(),           // reasonix
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
        let e = claude_build_entry("\"/x/hook.sh\" post-tool-use");
        assert_eq!(e["matcher"], "Edit|Write");
        assert_eq!(e["hooks"][0]["command"], "\"/x/hook.sh\" post-tool-use");
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
    fn embedded_set_covers_on_disk_adapter_files() {
        // Drift guard: every shell/ts file shipped under adapters/<h> for a
        // hook-capable harness must be embedded, else `hector init` ships a
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
