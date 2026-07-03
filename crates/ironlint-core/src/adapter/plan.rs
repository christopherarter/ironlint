//! Structured preview of what a harness install/uninstall touches. Core emits
//! this data; the CLI owns all formatting.
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanStep {
    /// A hook artifact (e.g. `hook.sh`) — or, for uninstall, the adapter
    /// directory that holds them.
    Hook { path: PathBuf },
    /// A plugin file (`ironlint.ts`).
    Plugin { path: PathBuf },
    /// A JSON settings patch: the file plus the hook-array key it lands in.
    Patch { path: PathBuf, key: &'static str },
    /// The `SKILL.md` authoring skill — or, for uninstall, its directory.
    Skill { path: PathBuf },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn patch_step_carries_key() {
        let s = PlanStep::Patch {
            path: PathBuf::from("/x/settings.json"),
            key: "PostToolUse",
        };
        match s {
            PlanStep::Patch { key, .. } => assert_eq!(key, "PostToolUse"),
            _ => panic!("wrong variant"),
        }
    }
}
