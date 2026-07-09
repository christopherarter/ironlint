use assert_cmd::Command;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn run_init(dir: &Path) {
    let xdg = tempdir().unwrap();
    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["init", "--dir", dir.to_str().unwrap()])
        .assert()
        .success();
}

fn read_cfg(dir: &Path) -> String {
    fs::read_to_string(dir.join(".ironlint.yml")).unwrap()
}

#[test]
fn init_scaffolds_checks_config_not_rules() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"foo\"\n").unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());
    assert!(
        cfg.starts_with("checks:\n"),
        "checks model config must start with `checks:`:\n{cfg}"
    );
    assert!(
        !cfg.contains("schema_version"),
        "checks model must not emit schema_version:\n{cfg}"
    );
    assert!(
        !cfg.contains("rules:"),
        "checks model must not emit rules: key:\n{cfg}"
    );
}

#[test]
fn init_existing_config_is_nonfatal_skipped() {
    // An existing .ironlint.yml must no longer be a hard error — init skips
    // scaffolding and prints a "already present (skipped)" note, but succeeds.
    let xdg = tempdir().unwrap();
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".ironlint.yml"), "existing\n").unwrap();
    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["init", "--dir", dir.path().to_str().unwrap(), "--no-hook"])
        .assert()
        .success()
        .stdout(predicates::str::contains("already present (skipped)"));
    // Original file content must be preserved.
    let content = fs::read_to_string(dir.path().join(".ironlint.yml")).unwrap();
    assert_eq!(content, "existing\n");
}

/// Generated config must validate with `ironlint validate`.
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
        let cfg_path = dir.path().join(".ironlint.yml");
        Command::cargo_bin("ironlint")
            .unwrap()
            .args(["validate", "--config", cfg_path.to_str().unwrap()])
            .assert()
            .code(0);
    }
}

/// Unknown stack (no manifest): universal baseline with no-fixme.
#[test]
fn init_unknown_stack_uses_generic_template() {
    let dir = tempdir().unwrap();
    run_init(dir.path());
    let cfg = read_cfg(dir.path());

    assert!(cfg.contains("no-fixme"));
}

#[test]
fn init_scaffolds_universal_baseline_regardless_of_stack() {
    for manifest in ["Cargo.toml", "package.json", "pyproject.toml", "none"] {
        let dir = tempfile::tempdir().unwrap();
        let xdg = tempfile::tempdir().unwrap();
        if manifest != "none" {
            std::fs::write(dir.path().join(manifest), "").unwrap();
        }
        Command::cargo_bin("ironlint")
            .unwrap()
            .env("XDG_CONFIG_HOME", xdg.path())
            .current_dir(dir.path())
            .args(["init", "--no-hook"])
            .assert()
            .success();
        let cfg = std::fs::read_to_string(dir.path().join(".ironlint.yml")).unwrap();
        assert!(
            cfg.contains("no-fixme:"),
            "{manifest}: missing no-fixme:\n{cfg}"
        );
        assert!(
            cfg.contains("no-merge-markers:"),
            "{manifest}: missing no-merge-markers:\n{cfg}"
        );
        assert!(
            cfg.contains("$IRONLINT_TMPFILE"),
            "{manifest}: missing tmpfile example:\n{cfg}"
        );
        // No toolchain-specific scaffolding.
        for tool in [
            "biome",
            "eslint",
            "ruff",
            "clippy",
            "no-unwrap",
            "console.log",
        ] {
            assert!(
                !cfg.contains(tool),
                "{manifest}: must not scaffold `{tool}`:\n{cfg}"
            );
        }
    }
}

#[test]
fn scaffolded_baseline_validates() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .current_dir(dir.path())
        .args(["init", "--no-hook"])
        .assert()
        .success();
    Command::cargo_bin("ironlint")
        .unwrap()
        .current_dir(dir.path())
        .args(["validate", "--config", ".ironlint.yml"])
        .assert()
        .success();
}

#[test]
fn init_dry_run_plans_skill_installs_for_explicit_harnesses() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let out = assert_cmd::Command::cargo_bin("ironlint")
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
    assert!(s.contains("ironlint · onboarding"), "plan header:\n{s}");
    assert!(s.contains("pi"), "must mention the pi harness:\n{s}");
    assert!(
        s.contains("requested"),
        "explicit harness tagged requested:\n{s}"
    );
    assert!(
        s.contains("skill"),
        "plan must include the skill step:\n{s}"
    );
    assert!(
        s.contains("skills/ironlint-config/SKILL.md"),
        "plan must name the skill path:\n{s}"
    );
}

#[test]
fn init_dedups_opencode_skill_when_claude_also_selected() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let out = assert_cmd::Command::cargo_bin("ironlint")
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
        s.contains(".claude/skills/ironlint-config/SKILL.md"),
        "claude-code skill must be planned:\n{s}"
    );
    assert!(
        !s.contains(".opencode/skills/ironlint-config"),
        "opencode skill must be deduped against claude's copy:\n{s}"
    );
}

/// Install-side counterpart to `init_dedups_opencode_skill_when_claude_also_selected`:
/// a REAL (non-dry-run) install must actually write claude-code's skill to
/// disk while actually skipping opencode's own skill dir (it reads claude's
/// copy), not just omit it from the printed plan.
#[test]
fn init_real_install_dedups_opencode_skill_against_claude() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    Command::cargo_bin("ironlint")
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
            "--yes",
        ])
        .assert()
        .success();

    let claude_skill = dir.path().join(".claude/skills/ironlint-config/SKILL.md");
    assert!(
        claude_skill.exists(),
        "claude-code skill must be installed to disk at {}",
        claude_skill.display()
    );

    let opencode_skill = dir.path().join(".opencode/skills/ironlint-config");
    assert!(
        !opencode_skill.exists(),
        "opencode's own skill dir must be deduped (not written) when claude-code \
         is also installed, since opencode reads claude's copy: {}",
        opencode_skill.display()
    );
}

/// `init` auto-blesses, so a `check` against the scaffolded config runs
/// without a separate `ironlint trust` step (it is not rejected as untrusted).
#[test]
fn init_auto_blesses_so_check_is_trusted() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();

    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["init", "--dir"])
        .arg(proj.path())
        .assert()
        .success();

    let cfg = proj.path().join(".ironlint.yml");
    let target = proj.path().join("a.rs");
    std::fs::write(&target, "x\n").unwrap();

    // Should NOT be rejected as untrusted. Some scaffolded gate may or may not
    // block on this file, but the verdict must not be the trust exit-1.
    let out = Command::cargo_bin("ironlint")
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
