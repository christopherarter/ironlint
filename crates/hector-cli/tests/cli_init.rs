use assert_cmd::Command;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn run_init(dir: &Path) {
    let xdg = tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["init", "--dir", dir.to_str().unwrap()])
        .assert()
        .success();
}

fn read_cfg(dir: &Path) -> String {
    fs::read_to_string(dir.join(".hector.yml")).unwrap()
}

#[test]
fn init_scaffolds_gates_config_not_rules() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"foo\"\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());
    assert!(
        cfg.starts_with("gates:\n"),
        "gates model config must start with `gates:`:\n{cfg}"
    );
    assert!(
        !cfg.contains("schema_version"),
        "gates model must not emit schema_version:\n{cfg}"
    );
    assert!(
        !cfg.contains("rules:"),
        "gates model must not emit rules: key:\n{cfg}"
    );
}

#[test]
fn init_scaffolds_for_rust_project() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"foo\"\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());
    assert!(cfg.contains("no-unwrap-in-src"));
    assert!(cfg.contains("gates:"));
}

#[test]
fn init_existing_config_is_nonfatal_skipped() {
    // An existing .hector.yml must no longer be a hard error — init skips
    // scaffolding and prints a "already present (skipped)" note, but succeeds.
    let xdg = tempdir().unwrap();
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".hector.yml"), "existing\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["init", "--dir", dir.path().to_str().unwrap(), "--no-hook"])
        .assert()
        .success()
        .stdout(predicates::str::contains("already present (skipped)"));
    // Original file content must be preserved.
    let content = fs::read_to_string(dir.path().join(".hector.yml")).unwrap();
    assert_eq!(content, "existing\n");
}

/// The grep-based gates must use `case $?` routing and exit 2 to block
/// (not `|| exit 0` masking, and not exit 1 which was the old convention).
#[test]
fn init_template_uses_exit_2_convention() {
    for (manifest, name, contents) in [
        ("Cargo.toml", "Cargo.toml", "[package]\nname = \"foo\"\n"),
        ("package.json", "package.json", "{}\n"),
        // Generic stack — no manifest.
        ("", "", ""),
    ] {
        let dir = tempdir().unwrap();
        if !manifest.is_empty() {
            fs::write(dir.path().join(name), contents).unwrap();
        }
        run_init(dir.path());
        let cfg = read_cfg(dir.path());

        assert!(
            !cfg.contains("|| exit 0"),
            "stack `{manifest}`: grep template must not mask exit 2 via `|| exit 0`; got:\n{cfg}"
        );
        assert!(
            cfg.contains("case $?"),
            "stack `{manifest}`: expected case-statement exit-code routing; got:\n{cfg}"
        );
        assert!(
            cfg.contains("exit 2"),
            "stack `{manifest}`: blocking arm must use exit 2; got:\n{cfg}"
        );
    }
}

/// Gates model uses $HECTOR_FILE env var, not {{file}} template.
#[test]
fn init_template_uses_hector_file_env_var() {
    for (manifest, name, contents) in [
        ("Cargo.toml", "Cargo.toml", "[package]\nname = \"foo\"\n"),
        ("package.json", "package.json", "{}\n"),
        ("", "", ""),
    ] {
        let dir = tempdir().unwrap();
        if !manifest.is_empty() {
            fs::write(dir.path().join(name), contents).unwrap();
        }
        run_init(dir.path());
        let cfg = read_cfg(dir.path());

        assert!(
            !cfg.contains("{{file}}") && !cfg.contains("{file}"),
            "stack `{manifest}`: must not use {{file}} template (use $HECTOR_FILE); got:\n{cfg}"
        );
        // Grep-based gates (rust, node, generic) use $HECTOR_FILE.
        // Python ruff gate uses $HECTOR_FILE too.
        assert!(
            cfg.contains("$HECTOR_FILE") || cfg.contains("HECTOR_FILE"),
            "stack `{manifest}`: gates must reference $HECTOR_FILE; got:\n{cfg}"
        );
    }
}

/// Generated config must validate with `hector validate`.
#[test]
fn init_generated_config_validates_ok() {
    for (manifest, name, contents) in [
        ("Cargo.toml", "Cargo.toml", "[package]\nname = \"foo\"\n"),
        ("package.json", "package.json", "{}\n"),
        (
            "pyproject.toml",
            "pyproject.toml",
            "[project]\nname=\"x\"\n",
        ),
        ("", "", ""),
    ] {
        let dir = tempdir().unwrap();
        if !manifest.is_empty() {
            fs::write(dir.path().join(name), contents).unwrap();
        }
        run_init(dir.path());
        let cfg_path = dir.path().join(".hector.yml");
        Command::cargo_bin("hector")
            .unwrap()
            .args(["validate", "--config", cfg_path.to_str().unwrap()])
            .assert()
            .code(0);
    }
}

// ---------------------------------------------------------------------
// Workspace + linter detection. Each test below is one scenario.
// ---------------------------------------------------------------------

/// Single-package npm project with no linter configured: scopes default to
/// `src/**/*.<ext>` patterns.
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
}

/// Single-package npm + biome: the `no-console-log` grep gate is dropped
/// (biome's `noConsole` catches it) and a `biome-check` gate is scaffolded instead.
#[test]
fn init_single_package_npm_with_biome_drops_console_log() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), "{}\n").unwrap();
    fs::write(dir.path().join("biome.json"), "{}\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(
        !cfg.contains("no-console-log"),
        "biome detected — `no-console-log` grep gate must be skipped; got:\n{cfg}"
    );
    assert!(
        cfg.contains("biome-check"),
        "biome detected — expected `biome-check` gate; got:\n{cfg}"
    );
    // No workspace manifests → scopes still `src/**/*.<ext>`.
    assert!(cfg.contains("src/**/*.ts"));
    // No pnpm-lock.yaml or yarn.lock → use npx as the package-manager exec.
    assert!(
        cfg.contains("npx biome"),
        "no lockfile → gate should use `npx`; got:\n{cfg}"
    );
}

/// Regression: the dynamic biome/eslint `run` strings carry embedded
/// double-quotes (e.g. `--stdin-file-path="$HECTOR_FILE"`) that must be escaped
/// so the scaffolded YAML stays valid under the STRICT parser. `hector validate`
/// runs that strict parser and is not trust-gated, so this guards the escaping
/// independently of the auto-bless wiring that also happens to catch it.
#[test]
fn init_biome_scaffold_strictly_validates() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), "{}\n").unwrap();
    fs::write(dir.path().join("biome.json"), "{}\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());
    assert!(
        cfg.contains("biome-check"),
        "expected the dynamic biome-check linter gate:\n{cfg}"
    );
    let cfg_path = dir.path().join(".hector.yml");
    Command::cargo_bin("hector")
        .unwrap()
        .args(["validate", "--config", cfg_path.to_str().unwrap()])
        .assert()
        .code(0);
}

/// pnpm workspace + biome: scopes use the workspace `packages:` globs,
/// `no-console-log` is dropped, and the wrapper uses `pnpm exec`.
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
    // Workspace globs land in files field.
    assert!(
        cfg.contains("apps/**/src/**/*.ts"),
        "pnpm workspace → expected `apps/**/src/**/*.ts` files; got:\n{cfg}"
    );
    assert!(
        cfg.contains("packages/**/src/**/*.ts"),
        "pnpm workspace → expected `packages/**/src/**/*.ts` files; got:\n{cfg}"
    );
    // Single-root `src/**` scope must NOT leak into monorepo configs.
    assert!(
        !cfg.contains("\"src/**/*.ts\""),
        "monorepo scaffold must not include single-root `src/**/*.ts`; got:\n{cfg}"
    );
    // pnpm-lock.yaml present → `pnpm exec`.
    assert!(
        cfg.contains("pnpm exec biome"),
        "pnpm-lock present → gate should use `pnpm exec`; got:\n{cfg}"
    );
}

/// pnpm workspace, no linter: the grep `no-console-log` gate still fires but
/// its scopes use the workspace shape.
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

/// Cargo workspace with clippy.toml present. The Rust grep gate scopes use
/// the workspace members.
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

    // The Rust unwrap gate stays — clippy.toml doesn't suppress it.
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

/// Single-package python: the `ruff-check` gate is scaffolded.
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
}

/// Unknown stack (no manifest): generic template.
#[test]
fn init_unknown_stack_uses_generic_template() {
    let dir = tempdir().unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(cfg.contains("no-fixme"));
}

/// Single-package npm + ESLint config (no biome): the `no-console-log` grep
/// gate is dropped and an `eslint-check` gate is scaffolded.
#[test]
fn init_single_package_npm_with_eslint_drops_console_log() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), "{}\n").unwrap();
    fs::write(dir.path().join(".eslintrc.json"), "{}\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(
        !cfg.contains("no-console-log"),
        "eslint detected — `no-console-log` grep gate must be skipped; got:\n{cfg}"
    );
    assert!(
        cfg.contains("eslint-check"),
        "eslint detected — expected `eslint-check` gate; got:\n{cfg}"
    );
    assert!(
        cfg.contains("npx eslint"),
        "no lockfile → gate should use `npx`; got:\n{cfg}"
    );
}

/// yarn lockfile selects `yarn exec` for the wrapper command.
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
        "yarn.lock present → gate should use `yarn exec`; got:\n{cfg}"
    );
}

/// Both biome AND eslint configured: prefer biome.
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
        "biome + eslint both present → prefer biome; got:\n{cfg}"
    );
}

/// pnpm-workspace.yaml without a quoted glob still parses.
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

#[test]
fn init_dry_run_plans_skill_installs_for_explicit_harnesses() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let out = assert_cmd::Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "init",
            "--dir",
            dir.path().to_str().unwrap(),
            "--harness",
            "pi",
            "--dry-run",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(
        s.contains("pi"),
        "dry-run output must mention the pi harness:\n{s}"
    );
    assert!(
        s.contains("skill dry-run"),
        "dry-run must plan a skill install:\n{s}"
    );
    assert!(
        s.contains("skills/hector-config/SKILL.md"),
        "dry-run must name the skill path:\n{s}"
    );
}

#[test]
fn init_dedups_opencode_skill_when_claude_also_selected() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let out = assert_cmd::Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "init",
            "--dir",
            dir.path().to_str().unwrap(),
            "--harness",
            "claude-code",
            "--harness",
            "opencode",
            "--dry-run",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    // Assert on paths (spacing-insensitive): claude's skill is planned;
    // opencode's own skill dir is not (it reads claude's copy).
    assert!(
        s.contains(".claude/skills/hector-config/SKILL.md"),
        "claude-code skill must be planned:\n{s}"
    );
    assert!(
        !s.contains(".opencode/skills/hector-config"),
        "opencode skill must be deduped against claude's copy:\n{s}"
    );
}

/// `init` auto-blesses, so a `check` against the scaffolded config runs
/// without a separate `hector trust` step (it is not rejected as untrusted).
#[test]
fn init_auto_blesses_so_check_is_trusted() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["init", "--dir"])
        .arg(proj.path())
        .assert()
        .success();

    let cfg = proj.path().join(".hector.yml");
    let target = proj.path().join("a.rs");
    std::fs::write(&target, "x\n").unwrap();

    // Should NOT be rejected as untrusted. Some scaffolded gate may or may not
    // block on this file, but the verdict must not be the trust exit-1.
    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["check", "--config"])
        .arg(&cfg)
        .arg("--file")
        .arg(&target)
        .assert();
    let code = out.get_output().status.code().unwrap();
    // Must be a real verdict — pass (0) or block (2) — never untrusted (1) and
    // never a crashed gate (3). `!= 1` alone would pass on an exit-3 regression.
    assert!(
        matches!(code, 0 | 2),
        "init-blessed config must run to a real verdict (0 or 2), not be \
         rejected as untrusted (1) or crash a gate (3); got {code}"
    );
}
