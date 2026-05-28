//! Workspace + linter detection for `hector init`.
//!
//! Detection is intentionally tolerant: missing/malformed manifests
//! degrade to "no workspace" or "no linter detected" rather than
//! erroring. Init is a UX surface — the worst case is the user gets
//! today's defaults, not a crash.
//!
//! See `docs/audits/2026-05-23-first-run-dx-audit.md#r1` for the spec.

use serde_yaml::Value as YamlValue;
use std::path::Path;

/// Resolved workspace shape — drives per-rule `scope:` generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub kind: WorkspaceKind,
    /// Package globs as written in the workspace manifest (e.g.
    /// `["apps/*", "packages/*"]`). Never empty; caller falls back to
    /// no-workspace when detection returns `None`.
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceKind {
    Pnpm,
    Npm,
    Cargo,
    Go,
}

/// Which JS package-manager `exec` invocation should the scaffolded
/// wrapper rule use? Detected from lockfile presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsRunner {
    Pnpm,
    Yarn,
    Npx,
}

impl JsRunner {
    pub fn exec_prefix(self) -> &'static str {
        match self {
            Self::Pnpm => "pnpm exec",
            Self::Yarn => "yarn exec",
            Self::Npx => "npx",
        }
    }
}

/// Detected linters in the project root. Order is irrelevant; callers
/// query by name. We never parse the linter's config — file presence
/// is enough.
//
// A presence set, one boolean per independently-detected linter — not a
// state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default, Clone, Copy)]
pub struct LinterSet {
    pub biome: bool,
    pub eslint: bool,
    pub ruff: bool,
    pub clippy: bool,
}

/// Detect the workspace shape from manifest files. Returns `None` for
/// single-package repos (caller falls back to `src/**/*.<ext>`).
pub fn detect_workspace(dir: &Path) -> Option<Workspace> {
    detect_pnpm(dir)
        .or_else(|| detect_npm(dir))
        .or_else(|| detect_cargo(dir))
        .or_else(|| detect_go(dir))
}

fn detect_pnpm(dir: &Path) -> Option<Workspace> {
    let raw = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).ok()?;
    let parsed: YamlValue = serde_yaml::from_str(&raw).ok()?;
    let packages = parsed
        .get("packages")
        .and_then(YamlValue::as_sequence)?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    (!packages.is_empty()).then_some(Workspace {
        kind: WorkspaceKind::Pnpm,
        packages,
    })
}

fn detect_npm(dir: &Path) -> Option<Workspace> {
    let raw = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let workspaces_field = parsed.get("workspaces")?;
    let packages = extract_npm_packages(workspaces_field)?;
    (!packages.is_empty()).then_some(Workspace {
        kind: WorkspaceKind::Npm,
        packages,
    })
}

fn extract_npm_packages(field: &serde_json::Value) -> Option<Vec<String>> {
    if let Some(arr) = field.as_array() {
        return Some(json_strings(arr));
    }
    let nested = field.as_object()?.get("packages")?.as_array()?;
    Some(json_strings(nested))
}

fn json_strings(arr: &[serde_json::Value]) -> Vec<String> {
    arr.iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

fn detect_cargo(dir: &Path) -> Option<Workspace> {
    // We don't want a heavy TOML dependency just for this; the
    // `[workspace] members = [...]` shape is regular enough to scan
    // textually.
    let raw = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let members = parse_cargo_members(&raw)?;
    (!members.is_empty()).then_some(Workspace {
        kind: WorkspaceKind::Cargo,
        packages: members,
    })
}

fn parse_cargo_members(raw: &str) -> Option<Vec<String>> {
    let ws_idx = raw.find("[workspace]")?;
    let after = &raw[ws_idx..];
    let members_idx = after.find("members")?;
    let rest = &after[members_idx..];
    let open = rest.find('[')?;
    let close = rest[open..].find(']')?;
    let inner = &rest[open + 1..open + close];
    let members = inner
        .split(',')
        .map(|s| s.trim().trim_matches(|c: char| c == '"' || c == '\''))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    Some(members)
}

fn detect_go(dir: &Path) -> Option<Workspace> {
    let raw = std::fs::read_to_string(dir.join("go.work")).ok()?;
    let modules = parse_go_use_block(&raw);
    (!modules.is_empty()).then_some(Workspace {
        kind: WorkspaceKind::Go,
        packages: modules,
    })
}

fn parse_go_use_block(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if !in_block {
            if let Some(rest) = trimmed.strip_prefix("use") {
                let rest = rest.trim_start();
                if let Some(single) = rest.strip_prefix("(").and_then(|s| s.strip_suffix(")")) {
                    push_go_modules(&mut out, single);
                } else if rest.starts_with('(') {
                    in_block = true;
                } else if !rest.is_empty() {
                    // `use ./module` single-line form
                    push_go_modules(&mut out, rest);
                }
            }
            continue;
        }
        if trimmed.starts_with(')') {
            in_block = false;
            continue;
        }
        push_go_modules(&mut out, trimmed);
    }
    out
}

fn push_go_modules(out: &mut Vec<String>, line: &str) {
    for token in line.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| c == '"' || c == '\'');
        if cleaned.is_empty() {
            continue;
        }
        out.push(cleaned.trim_start_matches("./").to_owned());
    }
}

/// Detect existing linter configs in the project root.
pub fn detect_linters(dir: &Path) -> LinterSet {
    LinterSet {
        biome: has_biome(dir),
        eslint: has_eslint(dir),
        ruff: has_ruff(dir),
        clippy: dir.join("clippy.toml").exists(),
    }
}

fn has_biome(dir: &Path) -> bool {
    ["biome.json", "biome.jsonc", "biome.json5"]
        .iter()
        .any(|name| dir.join(name).exists())
}

fn has_eslint(dir: &Path) -> bool {
    // Classic dotfile family + flat-config family.
    let candidates = [
        ".eslintrc",
        ".eslintrc.js",
        ".eslintrc.cjs",
        ".eslintrc.mjs",
        ".eslintrc.json",
        ".eslintrc.yaml",
        ".eslintrc.yml",
        "eslint.config.js",
        "eslint.config.cjs",
        "eslint.config.mjs",
        "eslint.config.ts",
    ];
    candidates.iter().any(|name| dir.join(name).exists())
}

fn has_ruff(dir: &Path) -> bool {
    if dir.join("ruff.toml").exists() {
        return true;
    }
    // `[tool.ruff]` block in pyproject.toml is the canonical signal.
    std::fs::read_to_string(dir.join("pyproject.toml"))
        .ok()
        .is_some_and(|s| s.contains("[tool.ruff"))
}

/// Detect the JS package-manager runner from lockfile presence.
pub fn detect_js_runner(dir: &Path) -> JsRunner {
    if dir.join("pnpm-lock.yaml").exists() {
        JsRunner::Pnpm
    } else if dir.join("yarn.lock").exists() {
        JsRunner::Yarn
    } else {
        JsRunner::Npx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn pnpm_detection_reads_packages_list() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - apps/*\n  - 'packages/*'\n",
        )
        .unwrap();
        let ws = detect_workspace(dir.path()).unwrap();
        assert_eq!(ws.kind, WorkspaceKind::Pnpm);
        assert_eq!(ws.packages, vec!["apps/*", "packages/*"]);
    }

    #[test]
    fn npm_workspaces_as_array_field() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            "{\"workspaces\":[\"apps/*\",\"packages/*\"]}",
        )
        .unwrap();
        let ws = detect_workspace(dir.path()).unwrap();
        assert_eq!(ws.kind, WorkspaceKind::Npm);
        assert_eq!(ws.packages, vec!["apps/*", "packages/*"]);
    }

    #[test]
    fn npm_workspaces_as_object_with_packages_key() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            "{\"workspaces\":{\"packages\":[\"apps/*\"]}}",
        )
        .unwrap();
        let ws = detect_workspace(dir.path()).unwrap();
        assert_eq!(ws.packages, vec!["apps/*"]);
    }

    #[test]
    fn npm_without_workspaces_field_returns_none() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert!(detect_workspace(dir.path()).is_none());
    }

    #[test]
    fn cargo_workspace_members_parsed() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/foo\", \"crates/bar\"]\n",
        )
        .unwrap();
        let ws = detect_workspace(dir.path()).unwrap();
        assert_eq!(ws.kind, WorkspaceKind::Cargo);
        assert_eq!(ws.packages, vec!["crates/foo", "crates/bar"]);
    }

    #[test]
    fn cargo_without_workspace_table_returns_none() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        assert!(detect_workspace(dir.path()).is_none());
    }

    #[test]
    fn go_work_use_block_parsed() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("go.work"),
            "go 1.22\n\nuse (\n  ./svc-a\n  ./svc-b\n)\n",
        )
        .unwrap();
        let ws = detect_workspace(dir.path()).unwrap();
        assert_eq!(ws.kind, WorkspaceKind::Go);
        assert_eq!(ws.packages, vec!["svc-a", "svc-b"]);
    }

    #[test]
    fn go_work_single_line_use_form() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("go.work"), "go 1.22\nuse ./svc\n").unwrap();
        let ws = detect_workspace(dir.path()).unwrap();
        assert_eq!(ws.packages, vec!["svc"]);
    }

    #[test]
    fn linter_detection_picks_up_biome_and_eslint() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("biome.json"), "{}").unwrap();
        std::fs::write(dir.path().join("eslint.config.mjs"), "").unwrap();
        let l = detect_linters(dir.path());
        assert!(l.biome);
        assert!(l.eslint);
        assert!(!l.ruff);
        assert!(!l.clippy);
    }

    #[test]
    fn linter_detection_picks_up_ruff_from_pyproject() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.ruff]\nline-length = 100\n",
        )
        .unwrap();
        let l = detect_linters(dir.path());
        assert!(l.ruff);
    }

    #[test]
    fn js_runner_falls_back_to_npx() {
        let dir = tempdir().unwrap();
        assert!(matches!(detect_js_runner(dir.path()), JsRunner::Npx));
    }

    #[test]
    fn js_runner_prefers_pnpm_over_yarn() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(dir.path().join("yarn.lock"), "").unwrap();
        assert!(matches!(detect_js_runner(dir.path()), JsRunner::Pnpm));
    }
}
