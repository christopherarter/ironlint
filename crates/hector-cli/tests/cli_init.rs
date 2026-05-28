use assert_cmd::Command;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn run_init(dir: &Path) {
    Command::cargo_bin("hector")
        .unwrap()
        .args(["init", "--dir", dir.to_str().unwrap()])
        .assert()
        .success();
}

fn read_cfg(dir: &Path) -> String {
    fs::read_to_string(dir.join(".hector.yml")).unwrap()
}

#[test]
fn init_scaffolds_for_rust_project() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"foo\"\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());
    assert!(cfg.contains("schema_version: 2"));
    assert!(cfg.contains("rules:"));
}

#[test]
fn init_refuses_to_overwrite() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".hector.yml"), "existing\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["init", "--dir", dir.path().to_str().unwrap()])
        .assert()
        .failure();
}

/// `grep PATTERN {file} && exit 1 || exit 0` collapses grep's exit-2
/// (regex/parse error) into exit 0 (pass), masking a broken rule forever.
/// The template routes exit codes through a `case` statement instead:
///   - 0 (found)        → exit 1 (violation)
///   - 1 (not found)    → exit 0 (pass)
///   - 2 (grep error)   → exit 2 (surfaced as violation by runner)
///
/// So the template must not contain `|| exit 0` (the masking idiom) and
/// must contain `case $?` (the explicit exit-code routing).
#[test]
fn init_template_preserves_grep_error_exit_codes() {
    for (manifest, name, contents) in [
        ("Cargo.toml", "Cargo.toml", "[package]\nname = \"foo\"\n"),
        ("package.json", "package.json", "{}\n"),
        (
            "pyproject.toml",
            "pyproject.toml",
            "[project]\nname=\"x\"\n",
        ),
        // Generic stack — no manifest.
        ("", "", ""),
    ] {
        let dir = tempdir().unwrap();
        if !manifest.is_empty() {
            fs::write(dir.path().join(name), contents).unwrap();
        }
        run_init(dir.path());
        let cfg = read_cfg(dir.path());

        // The Python template doesn't use grep; only assert on stacks that do.
        if manifest != "pyproject.toml" {
            assert!(
                !cfg.contains("|| exit 0"),
                "stack `{manifest}`: grep template must not mask exit 2 via `|| exit 0`; got:\n{cfg}"
            );
            assert!(
                cfg.contains("case $?"),
                "stack `{manifest}`: expected case-statement exit-code routing; got:\n{cfg}"
            );
        }
    }
}

// ---------------------------------------------------------------------
// Workspace + linter detection. Each test below is one scenario.
// ---------------------------------------------------------------------

/// Every generated config must end with a commented-out `llm:` block +
/// example semantic rule so the subagent path is discoverable without
/// reading source-repo docs. The block is at the END so it doesn't
/// clutter the active rules visually.
#[test]
fn init_appends_commented_llm_block_for_every_stack() {
    for (name, contents) in [
        ("Cargo.toml", "[package]\nname = \"foo\"\n"),
        ("package.json", "{}\n"),
        ("pyproject.toml", "[project]\nname=\"x\"\n"),
        ("", ""), // generic / unknown
    ] {
        let dir = tempdir().unwrap();
        if !name.is_empty() {
            fs::write(dir.path().join(name), contents).unwrap();
        }
        run_init(dir.path());
        let cfg = read_cfg(dir.path());
        assert!(
            cfg.contains("# llm:"),
            "stack `{name}`: expected commented `# llm:` block; got:\n{cfg}"
        );
        assert!(
            cfg.contains("claude-code-subagent"),
            "stack `{name}`: expected `claude-code-subagent` reference in LLM comment; got:\n{cfg}"
        );
        assert!(
            cfg.contains("no-todo-comment"),
            "stack `{name}`: expected example semantic rule `no-todo-comment` in LLM comment; got:\n{cfg}"
        );
        // The block sits at the end so it doesn't visually crowd active rules.
        let rules_idx = cfg
            .find("\nrules:")
            .unwrap_or_else(|| panic!("stack `{name}`: missing top-level `rules:` key in:\n{cfg}"));
        let llm_idx = cfg.find("# llm:").unwrap();
        assert!(
            llm_idx > rules_idx,
            "stack `{name}`: LLM comment block must come AFTER `rules:` (was {llm_idx} vs {rules_idx}) in:\n{cfg}"
        );
    }
}

/// Single-package npm project with no linter configured: scopes default to
/// `src/**/*.<ext>` plus the commented LLM block.
#[test]
fn init_single_package_npm_no_linter() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), "{}\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(cfg.contains("no-console-log"));
    assert!(cfg.contains("src/**/*.ts"));
    assert!(!cfg.contains("biome-check"));
    assert!(!cfg.contains("eslint-check"));
    assert!(cfg.contains("# llm:"));
}

/// Single-package npm + biome: the `no-console-log` grep rule is dropped
/// (biome's `noConsole` catches it) and a passthrough `biome-check` wrapper
/// is scaffolded instead.
#[test]
fn init_single_package_npm_with_biome_drops_console_log() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), "{}\n").unwrap();
    fs::write(dir.path().join("biome.json"), "{}\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(
        !cfg.contains("no-console-log"),
        "biome detected — `no-console-log` grep rule must be skipped; got:\n{cfg}"
    );
    assert!(
        cfg.contains("biome-check"),
        "biome detected — expected `biome-check` passthrough rule; got:\n{cfg}"
    );
    assert!(
        cfg.contains("output: passthrough"),
        "passthrough wrapper rules must explicitly opt in to `output: passthrough`; got:\n{cfg}"
    );
    // No workspace manifests → scopes still `src/**/*.<ext>`.
    assert!(cfg.contains("src/**/*.ts"));
    // No pnpm-lock.yaml or yarn.lock → use npx as the package-manager exec.
    assert!(
        cfg.contains("npx biome"),
        "no lockfile → wrapper should use `npx`; got:\n{cfg}"
    );
}

/// pnpm workspace + biome: scopes use the workspace `packages:` globs,
/// `no-console-log` is dropped, and the wrapper uses `pnpm exec` because
/// `pnpm-lock.yaml` exists.
#[test]
fn init_pnpm_workspace_with_biome_uses_workspace_scopes() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), "{\"name\":\"root\"}\n").unwrap();
    fs::write(
        dir.path().join("pnpm-workspace.yaml"),
        "packages:\n  - 'apps/*'\n  - 'packages/*'\n",
    )
    .unwrap();
    fs::write(dir.path().join("pnpm-lock.yaml"), "lockfileVersion: 9\n").unwrap();
    fs::write(dir.path().join("biome.json"), "{}\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(!cfg.contains("no-console-log"));
    assert!(cfg.contains("biome-check"));
    // Workspace globs land in scope.
    assert!(
        cfg.contains("apps/**/src/**/*.ts"),
        "pnpm workspace → expected `apps/**/src/**/*.ts` scope; got:\n{cfg}"
    );
    assert!(
        cfg.contains("packages/**/src/**/*.ts"),
        "pnpm workspace → expected `packages/**/src/**/*.ts` scope; got:\n{cfg}"
    );
    // Single-root `src/**` scope must NOT leak into monorepo configs.
    assert!(
        !cfg.contains("\"src/**/*.ts\""),
        "monorepo scaffold must not include single-root `src/**/*.ts`; got:\n{cfg}"
    );
    // pnpm-lock.yaml present → `pnpm exec`.
    assert!(
        cfg.contains("pnpm exec biome"),
        "pnpm-lock present → wrapper should use `pnpm exec`; got:\n{cfg}"
    );
}

/// pnpm workspace, no linter: the grep `no-console-log` rule still fires but
/// its scopes use the workspace shape, not a single-root `src/`.
#[test]
fn init_pnpm_workspace_no_linter_uses_workspace_scopes() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), "{\"name\":\"root\"}\n").unwrap();
    fs::write(
        dir.path().join("pnpm-workspace.yaml"),
        "packages:\n  - 'apps/*'\n",
    )
    .unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(cfg.contains("no-console-log"));
    assert!(
        cfg.contains("apps/**/src/**/*.ts"),
        "expected workspace-shaped scope; got:\n{cfg}"
    );
    assert!(
        !cfg.contains("\"src/**/*.ts\""),
        "monorepo scaffold must not include single-root `src/**/*.ts`; got:\n{cfg}"
    );
}

/// Cargo workspace with clippy.toml present. The Rust grep rule scopes use
/// the workspace members glob (Rust workspaces often don't use `src/` at the
/// top), and clippy.toml does NOT cause us to drop the unwrap grep rule.
#[test]
fn init_cargo_workspace_scopes_match_members() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/foo\", \"crates/bar\"]\n",
    )
    .unwrap();
    fs::write(dir.path().join("clippy.toml"), "msrv = \"1.75\"\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    // The Rust unwrap rule stays — clippy.toml doesn't suppress it
    // (clippy is repo-wide).
    assert!(cfg.contains("no-unwrap-in-src"));
    // Cargo workspaces don't have a single-root src/ — scope should
    // reference workspace members.
    assert!(
        cfg.contains("crates/foo/**/*.rs") || cfg.contains("crates/**/*.rs"),
        "Cargo workspace → expected member-shaped scope; got:\n{cfg}"
    );
    assert!(
        !cfg.contains("\"src/**/*.rs\""),
        "Cargo workspace scaffold must not use single-root `src/**/*.rs`; got:\n{cfg}"
    );
}

/// Single-package python with `[tool.ruff]`: the `ruff-check` template stays —
/// its scope is `**/*.py`, which already works for non-monorepo Python.
#[test]
fn init_python_with_ruff_keeps_template() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname=\"x\"\n[tool.ruff]\nline-length=100\n",
    )
    .unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(cfg.contains("ruff-check"));
    assert!(cfg.contains("**/*.py"));
    assert!(cfg.contains("# llm:"));
}

/// Unknown stack (no manifest): generic template plus the LLM comment block.
#[test]
fn init_unknown_stack_uses_generic_template() {
    let dir = tempdir().unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(cfg.contains("no-fixme"));
    assert!(cfg.contains("# llm:"));
}

/// Single-package npm + ESLint config (no biome): the `no-console-log` grep
/// rule is dropped (eslint's `no-console` covers it) and an `eslint-check`
/// passthrough wrapper is scaffolded.
#[test]
fn init_single_package_npm_with_eslint_drops_console_log() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), "{}\n").unwrap();
    fs::write(dir.path().join(".eslintrc.json"), "{}\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(
        !cfg.contains("no-console-log"),
        "eslint detected — `no-console-log` grep rule must be skipped; got:\n{cfg}"
    );
    assert!(
        cfg.contains("eslint-check"),
        "eslint detected — expected `eslint-check` passthrough rule; got:\n{cfg}"
    );
    assert!(cfg.contains("output: passthrough"));
    assert!(
        cfg.contains("npx eslint"),
        "no lockfile → wrapper should use `npx`; got:\n{cfg}"
    );
}

/// Bonus: yarn lockfile selects `yarn exec` for the wrapper command.
#[test]
fn init_with_yarn_lock_uses_yarn_exec() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), "{}\n").unwrap();
    fs::write(dir.path().join("yarn.lock"), "# yarn\n").unwrap();
    fs::write(dir.path().join("biome.json"), "{}\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(
        cfg.contains("yarn exec biome"),
        "yarn.lock present → wrapper should use `yarn exec`; got:\n{cfg}"
    );
}

/// Both biome AND eslint configured is unusual (typically during a
/// migration). Resolve in favor of biome — the more modern tool. Either
/// way, neither produces a duplicate `no-console-log` grep rule.
#[test]
fn init_with_both_biome_and_eslint_prefers_biome() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), "{}\n").unwrap();
    fs::write(dir.path().join("biome.json"), "{}\n").unwrap();
    fs::write(dir.path().join(".eslintrc.json"), "{}\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(!cfg.contains("no-console-log"));
    assert!(cfg.contains("biome-check"));
    assert!(
        !cfg.contains("eslint-check"),
        "biome + eslint both present → prefer biome, do not scaffold both wrappers; got:\n{cfg}"
    );
}

/// pnpm-workspace.yaml without a quoted glob still parses. (YAML allows
/// bare scalars; some pnpm configs use unquoted entries.)
#[test]
fn init_pnpm_workspace_handles_unquoted_globs() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), "{\"name\":\"root\"}\n").unwrap();
    fs::write(
        dir.path().join("pnpm-workspace.yaml"),
        "packages:\n  - apps/*\n  - packages/*\n",
    )
    .unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(cfg.contains("apps/**/src/**/*.ts"));
    assert!(cfg.contains("packages/**/src/**/*.ts"));
}

/// npm `workspaces` field as an array.
#[test]
fn init_npm_workspaces_array_field() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("package.json"),
        "{\"name\":\"root\",\"workspaces\":[\"apps/*\",\"packages/*\"]}\n",
    )
    .unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(cfg.contains("apps/**/src/**/*.ts"));
    assert!(cfg.contains("packages/**/src/**/*.ts"));
}

/// npm `workspaces` field as an object with `packages:` key.
#[test]
fn init_npm_workspaces_object_field() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("package.json"),
        "{\"name\":\"root\",\"workspaces\":{\"packages\":[\"apps/*\"]}}\n",
    )
    .unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(cfg.contains("apps/**/src/**/*.ts"));
}
