//! `hector init` — scaffold a starter `.hector.yml`.
//!
//! Detects workspace shape (pnpm / npm / cargo / go.work) and existing
//! linters (biome / eslint / ruff / clippy) to produce a config that
//! matches real-world repos rather than a generic `src/**` template.
//!
//! See `docs/audits/2026-05-23-first-run-dx-audit.md#r1` for the full
//! design rationale.

mod detect;

use anyhow::{anyhow, Result};
use detect::{detect_js_runner, detect_linters, detect_workspace, JsRunner, LinterSet, Workspace};
use std::fmt::Write as _;
use std::path::Path;

pub fn run(dir: &Path) -> Result<i32> {
    let cfg_path = dir.join(".hector.yml");
    if cfg_path.exists() {
        return Err(anyhow!(
            "{} already exists; refusing to overwrite",
            cfg_path.display()
        ));
    }
    let stack = detect_stack(dir);
    let workspace = detect_workspace(dir);
    let linters = detect_linters(dir);
    let runner = detect_js_runner(dir);
    let body = build_config(stack, workspace.as_ref(), linters, runner);
    std::fs::write(&cfg_path, body)?;
    println!("scaffolded: {}", cfg_path.display());
    println!("review the config, then run: hector trust");
    Ok(0)
}

#[derive(Debug, Clone, Copy)]
enum Stack {
    Rust,
    Node,
    Python,
    Unknown,
}

fn detect_stack(dir: &Path) -> Stack {
    if dir.join("Cargo.toml").exists() {
        return Stack::Rust;
    }
    if dir.join("package.json").exists() {
        return Stack::Node;
    }
    if dir.join("pyproject.toml").exists() || dir.join("setup.py").exists() {
        return Stack::Python;
    }
    Stack::Unknown
}

/// Top-level template assembly, dispatching to one emitter per stack.
fn build_config(
    stack: Stack,
    workspace: Option<&Workspace>,
    linters: LinterSet,
    runner: JsRunner,
) -> String {
    let mut out = String::from("schema_version: 2\n\nrules:\n");
    match stack {
        Stack::Rust => emit_rust_rules(&mut out, workspace, linters),
        Stack::Node => emit_node_rules(&mut out, workspace, linters, runner),
        Stack::Python => emit_python_rules(&mut out, workspace, linters),
        Stack::Unknown => emit_generic_rules(&mut out),
    }
    out.push_str(LLM_COMMENT_BLOCK);
    out
}

// --- per-stack rule assemblers ------------------------------------

fn emit_rust_rules(out: &mut String, workspace: Option<&Workspace>, linters: LinterSet) {
    // Cargo workspaces often don't have a single-root `src/` — most
    // members nest their own. Use a `<member>/**/*.rs` pattern so the
    // grep rule matches at any depth inside the member.
    let scopes = scope_list_with_default(workspace, &[".rs"], "src/**/*.rs", "/**/*.rs");
    if linters.clippy {
        // Detected but not auto-scaffolded: `cargo clippy` is repo-wide
        // (not per-file), so it belongs in a session rule rather than
        // a per-edit script rule. Leave a breadcrumb for the user.
        out.push_str(
            "  # clippy.toml detected. `cargo clippy` is repo-wide; see\n  # docs/engines.md for adding it as a session rule.\n",
        );
    }
    let _ = writeln!(
        out,
        "  no-unwrap-in-src:\n    description: \"Avoid .unwrap() in non-test source. Use ? or expect with context.\"\n    engine: script\n    scope: [{scopes}]\n    severity: warning\n    script: \"grep -nE '\\\\.unwrap\\\\(\\\\)' {{file}}; case $? in 0) exit 1;; 1) exit 0;; *) exit $?;; esac\""
    );
    out.push('\n');
}

fn emit_node_rules(
    out: &mut String,
    workspace: Option<&Workspace>,
    linters: LinterSet,
    runner: JsRunner,
) {
    let exts = [".ts", ".tsx", ".js"];
    let scopes = scope_list_with_default(workspace, &exts, "src/**/*.{ext}", "/src/**/*.{ext}");

    // Biome subsumes the noConsole check. Eslint's no-console rule does
    // the same. Both → skip the duplicate grep rule and emit a
    // passthrough wrapper instead. Biome wins when both are present;
    // see audit note in the test file.
    if linters.biome {
        emit_passthrough_wrapper(
            out,
            "biome-check",
            "File must pass biome check.",
            &scopes,
            &format!(
                "{} biome check --no-errors-on-unmatched {{file}}",
                runner.exec_prefix()
            ),
        );
    } else if linters.eslint {
        emit_passthrough_wrapper(
            out,
            "eslint-check",
            "File must pass eslint.",
            &scopes,
            &format!(
                "{} eslint --no-error-on-unmatched-pattern {{file}}",
                runner.exec_prefix()
            ),
        );
    } else {
        let _ = writeln!(
            out,
            "  no-console-log:\n    description: \"No console.log in committed source.\"\n    engine: script\n    scope: [{scopes}]\n    severity: error\n    script: \"grep -nE 'console\\\\.log\\\\(' {{file}}; case $? in 0) exit 1;; 1) exit 0;; *) exit $?;; esac\""
        );
        out.push('\n');
    }
}

fn emit_python_rules(out: &mut String, _workspace: Option<&Workspace>, linters: LinterSet) {
    // Today's ruff-check template — scope `**/*.py` already works for
    // both single-package and monorepo Python. Detection is here for
    // symmetry; future work could narrow scope to workspace members.
    if linters.ruff {
        out.push_str("  # ruff configuration detected; the rule below shells out to it.\n");
    }
    let _ = writeln!(
        out,
        "  ruff-check:\n    description: \"Code must pass ruff check.\"\n    engine: script\n    scope: [\"**/*.py\"]\n    severity: error\n    script: \"ruff check --quiet {{file}}\""
    );
    out.push('\n');
}

fn emit_generic_rules(out: &mut String) {
    let _ = writeln!(
        out,
        "  no-fixme:\n    description: \"Don't commit FIXME markers.\"\n    engine: script\n    scope: [\"*\"]\n    severity: warning\n    script: \"grep -nE 'FIXME' {{file}}; case $? in 0) exit 1;; 1) exit 0;; *) exit $?;; esac\""
    );
    out.push('\n');
}

fn emit_passthrough_wrapper(
    out: &mut String,
    rule_id: &str,
    description: &str,
    scopes: &str,
    script: &str,
) {
    let _ = writeln!(
        out,
        "  {rule_id}:\n    description: \"{description}\"\n    engine: script\n    scope: [{scopes}]\n    severity: error\n    output: passthrough\n    script: \"{script}\""
    );
    out.push('\n');
}

// --- scope generation --------------------------------------------

/// Produce a comma-separated, double-quoted scope list. Falls back to
/// the single-root template when no workspace is detected.
///
/// `workspace_suffix` is appended to each workspace package glob; it
/// uses `{ext}` as the extension placeholder. For Rust workspaces
/// where there's no per-member `src/`, use `"/**/*.rs"` directly (no
/// `{ext}` substitution).
fn scope_list_with_default(
    workspace: Option<&Workspace>,
    extensions: &[&str],
    single_root_template: &str,
    workspace_suffix: &str,
) -> String {
    if let Some(ws) = workspace {
        return workspace_scopes(ws, extensions, workspace_suffix);
    }
    single_root_scopes(extensions, single_root_template)
}

fn single_root_scopes(extensions: &[&str], template: &str) -> String {
    extensions
        .iter()
        .map(|ext| {
            let stripped = ext.strip_prefix('.').unwrap_or(ext);
            format!("\"{}\"", template.replace("{ext}", stripped))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn workspace_scopes(ws: &Workspace, extensions: &[&str], suffix: &str) -> String {
    let mut out = Vec::with_capacity(ws.packages.len() * extensions.len().max(1));
    for pkg in &ws.packages {
        let base = expand_package_glob(pkg);
        // `extensions == &[]` is the "no-extension" case (Rust today).
        if extensions.is_empty() {
            out.push(format!("\"{base}{suffix}\""));
            continue;
        }
        for ext in extensions {
            let stripped = ext.strip_prefix('.').unwrap_or(ext);
            let expanded_suffix = suffix.replace("{ext}", stripped);
            out.push(format!("\"{base}{expanded_suffix}\""));
        }
    }
    out.join(", ")
}

/// Convert a workspace package glob (`apps/*`, `crates/foo`) into a
/// scope-friendly prefix:
///   - `apps/*`     → `apps/**`
///   - `crates/foo` → `crates/foo`
///
/// The trailing `*` in pnpm/npm workspace globs is the "any direct
/// child" convention; we widen it to `**` so scopes match nested
/// sources at any depth.
fn expand_package_glob(pkg: &str) -> String {
    if let Some(prefix) = pkg.strip_suffix("/*") {
        format!("{prefix}/**")
    } else {
        pkg.to_owned()
    }
}

// --- commented LLM block ----------------------------------------

const LLM_COMMENT_BLOCK: &str = r#"
# Uncomment to enable LLM-driven semantic rules. The `claude-code-subagent`
# provider routes evaluation through your Claude Code session — no API key
# needed. See docs/reference/emit-semantic-payload.md.
#
# llm:
#   provider: claude-code-subagent
#   # evaluator_model: haiku   # optional; see adapter README
#
# rules:
#   no-todo-comment:
#     description: |
#       Source files must not contain TODO/FIXME/XXX comments —
#       track work in issues, not in code.
#     engine: semantic
#     scope: ["**/*"]                    # commented example; adjust to your stack
#     severity: warning
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(kind: detect::WorkspaceKind, pkgs: &[&str]) -> Workspace {
        Workspace {
            kind,
            packages: pkgs.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn expand_package_glob_widens_trailing_star() {
        assert_eq!(expand_package_glob("apps/*"), "apps/**");
        assert_eq!(expand_package_glob("crates/foo"), "crates/foo");
    }

    #[test]
    fn workspace_scopes_emits_each_extension_per_package() {
        let w = ws(detect::WorkspaceKind::Pnpm, &["apps/*"]);
        let s = workspace_scopes(&w, &[".ts", ".tsx"], "/src/**/*.{ext}");
        assert!(s.contains("\"apps/**/src/**/*.ts\""));
        assert!(s.contains("\"apps/**/src/**/*.tsx\""));
    }

    #[test]
    fn workspace_scopes_with_no_extensions_uses_suffix_verbatim() {
        let w = ws(detect::WorkspaceKind::Cargo, &["crates/foo"]);
        let s = workspace_scopes(&w, &[], "/**/*.rs");
        assert_eq!(s, "\"crates/foo/**/*.rs\"");
    }

    #[test]
    fn single_root_template_substitutes_extension() {
        let s = single_root_scopes(&[".ts", ".js"], "src/**/*.{ext}");
        assert!(s.contains("\"src/**/*.ts\""));
        assert!(s.contains("\"src/**/*.js\""));
    }
}
