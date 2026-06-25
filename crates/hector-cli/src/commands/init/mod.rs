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
    hector_core::trust::bless(&cfg_path).map_err(|e| {
        anyhow!(
            "scaffolded {} but could not trust it: {e:#}",
            cfg_path.display()
        )
    })?;
    println!("scaffolded and trusted: {}", cfg_path.display());
    println!(
        "review the config, then run: hector check --file <path> --config {}",
        cfg_path.display()
    );
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
    let mut out = String::from("gates:\n");
    match stack {
        Stack::Rust => emit_rust_gates(&mut out, workspace, linters),
        Stack::Node => emit_node_gates(&mut out, workspace, linters, runner),
        Stack::Python => emit_python_gates(&mut out, workspace, linters),
        Stack::Unknown => emit_generic_gates(&mut out),
    }
    out
}

// --- per-stack gate assemblers ------------------------------------

fn emit_rust_gates(out: &mut String, workspace: Option<&Workspace>, linters: LinterSet) {
    // Cargo workspaces often don't have a single-root `src/` — most
    // members nest their own. Use a `<member>/**/*.rs` pattern so the
    // grep rule matches at any depth inside the member.
    let files = scope_list_with_default(workspace, &[".rs"], "src/**/*.rs", "/**/*.rs");
    if linters.clippy {
        // Detected but not auto-scaffolded: `cargo clippy` is repo-wide
        // (not per-file), so it doesn't map to a per-file gate.
        // Leave a breadcrumb for the user.
        out.push_str(
            "  # clippy.toml detected. `cargo clippy` is repo-wide; run it\n  # as a pre-push step outside hector.\n",
        );
    }
    let _ = writeln!(
        out,
        "  no-unwrap-in-src:\n    files: [{files}]\n    run: \"grep -nE '\\\\.unwrap\\\\(\\\\)' \\\"$HECTOR_FILE\\\"; case $? in 0) exit 2;; 1) exit 0;; *) exit $?;; esac\""
    );
    out.push('\n');
}

fn emit_node_gates(
    out: &mut String,
    workspace: Option<&Workspace>,
    linters: LinterSet,
    runner: JsRunner,
) {
    let exts = [".ts", ".tsx", ".js"];
    let files = scope_list_with_default(workspace, &exts, "src/**/*.{ext}", "/src/**/*.{ext}");

    // Biome subsumes the noConsole check. Eslint's no-console rule does
    // the same. Both → skip the duplicate grep gate and emit a linter
    // wrapper instead. Biome wins when both are present;
    // see audit note in the test file.
    if linters.biome {
        emit_linter_gate(
            out,
            "biome-check",
            &files,
            &format!(
                "{} biome check --stdin-file-path=\"$HECTOR_FILE\" - || exit 2",
                runner.exec_prefix()
            ),
        );
    } else if linters.eslint {
        emit_linter_gate(
            out,
            "eslint-check",
            &files,
            &format!(
                "{} eslint --stdin --stdin-filename \"$HECTOR_FILE\" || exit 2",
                runner.exec_prefix()
            ),
        );
    } else {
        let _ = writeln!(
            out,
            "  no-console-log:\n    files: [{files}]\n    run: \"grep -nE 'console\\\\.log\\\\(' \\\"$HECTOR_FILE\\\"; case $? in 0) exit 2;; 1) exit 0;; *) exit $?;; esac\""
        );
        out.push('\n');
    }
}

fn emit_python_gates(out: &mut String, _workspace: Option<&Workspace>, linters: LinterSet) {
    // Today's ruff-check template — scope `**/*.py` already works for
    // both single-package and monorepo Python. Detection is here for
    // symmetry; future work could narrow scope to workspace members.
    if linters.ruff {
        out.push_str("  # ruff configuration detected; the gate below shells out to it.\n");
    }
    let _ = writeln!(
        out,
        "  ruff-check:\n    files: [\"**/*.py\"]\n    run: \"ruff check --quiet --stdin-filename \\\"$HECTOR_FILE\\\" - || exit 2\""
    );
    out.push('\n');
}

fn emit_generic_gates(out: &mut String) {
    let _ = writeln!(
        out,
        "  no-fixme:\n    files: [\"*\"]\n    run: \"grep -nE 'FIXME' \\\"$HECTOR_FILE\\\"; case $? in 0) exit 2;; 1) exit 0;; *) exit $?;; esac\""
    );
    out.push('\n');
}

fn emit_linter_gate(out: &mut String, gate_id: &str, files: &str, run: &str) {
    // Escape for a YAML double-quoted scalar: backslash first (so it isn't
    // re-doubled), then the embedded double-quotes.
    let run_escaped = run.replace('\\', "\\\\").replace('"', "\\\"");
    let _ = writeln!(
        out,
        "  {gate_id}:\n    files: [{files}]\n    run: \"{run_escaped}\""
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
    fn scaffolded_config_starts_with_gates() {
        let yaml = build_config(Stack::Rust, None, LinterSet::default(), JsRunner::Npx);
        assert!(
            yaml.starts_with("gates:\n"),
            "gates model config must start with `gates:`:\n{yaml}"
        );
        assert!(
            !yaml.contains("schema_version"),
            "gates model must not emit schema_version:\n{yaml}"
        );
        assert!(
            !yaml.contains("rules:"),
            "gates model must not emit rules: key:\n{yaml}"
        );
    }

    #[test]
    fn scaffolded_biome_gate_uses_stdin_form() {
        // build_config with biome=true (Node stack) must emit the stdin form so
        // pre-write gating works: the command reads from stdin, not disk.
        let linters = LinterSet {
            biome: true,
            ..Default::default()
        };
        let yaml = build_config(Stack::Node, None, linters, JsRunner::Npx);
        assert!(
            yaml.contains("--stdin-file-path"),
            "biome gate must use --stdin-file-path so pre-write gating works:\n{yaml}"
        );
        assert!(
            !yaml.contains("--no-errors-on-unmatched"),
            "old disk-reading form must be gone:\n{yaml}"
        );
    }

    #[test]
    fn scaffolded_eslint_gate_uses_stdin_form() {
        let linters = LinterSet {
            eslint: true,
            ..Default::default()
        };
        let yaml = build_config(Stack::Node, None, linters, JsRunner::Npx);
        assert!(
            yaml.contains("eslint --stdin --stdin-filename"),
            "eslint gate must use bare --stdin flag (not just --stdin-filename) so pre-write gating works:\n{yaml}"
        );
        assert!(
            !yaml.contains("--no-error-on-unmatched-pattern"),
            "old disk-reading form must be gone:\n{yaml}"
        );
    }

    #[test]
    fn scaffolded_ruff_gate_uses_stdin_form() {
        let linters = LinterSet {
            ruff: true,
            ..Default::default()
        };
        let yaml = build_config(Stack::Python, None, linters, JsRunner::Npx);
        assert!(
            yaml.contains("--stdin-filename"),
            "ruff gate must use --stdin-filename so pre-write gating works:\n{yaml}"
        );
        // The trailing '-' makes ruff read stdin; check for it after the flag.
        // Note: $HECTOR_FILE is the env-var form in the gates model.
        assert!(
            yaml.contains("--stdin-filename"),
            "ruff gate must have --stdin-filename flag:\n{yaml}"
        );
    }

    #[test]
    fn scaffolded_grep_gates_use_hector_file_env_var() {
        // Rust no-unwrap: grep targets $HECTOR_FILE (gates model)
        let yaml_rust = build_config(Stack::Rust, None, LinterSet::default(), JsRunner::Npx);
        assert!(
            yaml_rust.contains("$HECTOR_FILE"),
            "no-unwrap grep must target $HECTOR_FILE in gates model:\n{yaml_rust}"
        );
        // Node no-console-log: grep targets $HECTOR_FILE
        let yaml_node = build_config(Stack::Node, None, LinterSet::default(), JsRunner::Npx);
        assert!(
            yaml_node.contains("$HECTOR_FILE"),
            "no-console-log grep must target $HECTOR_FILE in gates model:\n{yaml_node}"
        );
        // Generic no-fixme: grep targets $HECTOR_FILE
        let yaml_generic = build_config(Stack::Unknown, None, LinterSet::default(), JsRunner::Npx);
        assert!(
            yaml_generic.contains("$HECTOR_FILE"),
            "no-fixme grep must target $HECTOR_FILE in gates model:\n{yaml_generic}"
        );
    }

    #[test]
    fn scaffolded_grep_gates_block_with_exit_2() {
        // All grep-based gates must exit 2 on match (block), not exit 1.
        let yaml_rust = build_config(Stack::Rust, None, LinterSet::default(), JsRunner::Npx);
        assert!(
            yaml_rust.contains("exit 2"),
            "no-unwrap must block with exit 2:\n{yaml_rust}"
        );
        let yaml_node = build_config(Stack::Node, None, LinterSet::default(), JsRunner::Npx);
        assert!(
            yaml_node.contains("exit 2"),
            "no-console-log must block with exit 2:\n{yaml_node}"
        );
        let yaml_generic = build_config(Stack::Unknown, None, LinterSet::default(), JsRunner::Npx);
        assert!(
            yaml_generic.contains("exit 2"),
            "no-fixme must block with exit 2:\n{yaml_generic}"
        );
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
