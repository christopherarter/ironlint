mod common;

use assert_cmd::Command;
use predicates::str::contains;
use std::fs;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/components")).unwrap();
    fs::create_dir_all(dir.path().join("src/data")).unwrap();
    fs::write(dir.path().join("src/data/db.ts"), "export const db = 1;\n").unwrap();

    let config = dir.path().join(".ironlint.yml");
    let config_body = "architecture:\n  layers:\n    - name: presentation\n      globs: [\"src/components/**\"]\n    - name: data\n      globs: [\"src/data/**\"]\n  rules:\n    - from: presentation\n      may_import: []\nchecks: {}\n";
    fs::write(&config, config_body).unwrap();

    let app = dir.path().join("src/components/App.tsx");
    (dir, config, app)
}

fn ironlint_path() -> String {
    let bin = assert_cmd::cargo::cargo_bin("ironlint");
    let bin_dir = bin.parent().unwrap();
    format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

#[test]
fn arch_check_blocks_forbidden_import_through_ironlint_check() {
    let (dir, config, app) = fixture();
    fs::write(&app, "export function App() { return null; }\n").unwrap();
    let xdg = common::blessed_store(&config);

    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .env("PATH", ironlint_path())
        .args([
            "check",
            "--file",
            app.to_str().unwrap(),
            "--content",
            "-",
            "--config",
            ".ironlint.yml",
        ]);
    cmd.write_stdin("import { db } from '../data/db';\nexport { db };\n")
        .assert()
        .code(2)
        .stderr(contains("presentation"));
}

#[test]
fn arch_check_passes_clean_content() {
    let (dir, config, app) = fixture();
    fs::write(
        &app,
        "import { helper } from './helper';\nexport { helper };\n",
    )
    .unwrap();
    let xdg = common::blessed_store(&config);

    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .env("PATH", ironlint_path())
        .args([
            "check",
            "--file",
            app.to_str().unwrap(),
            "--content",
            "-",
            "--config",
            ".ironlint.yml",
        ]);
    cmd.write_stdin("import { helper } from './helper';\nexport { helper };\n")
        .assert()
        .code(0);
}

#[test]
fn arch_check_blocks_on_disk_forbidden_import() {
    let (dir, config, app) = fixture();
    fs::write(&app, "import { db } from '../data/db';\nexport { db };\n").unwrap();
    let xdg = common::blessed_store(&config);

    Command::cargo_bin("ironlint")
        .unwrap()
        .current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .env("PATH", ironlint_path())
        .args(["check", "--config", ".ironlint.yml"])
        .assert()
        .code(2)
        .stderr(contains("presentation"));
}

/// Regression for Bug 6: a NESTED relative --config path (e.g. run from the
/// repo root with `--config packages/app/.ironlint.yml`) must pass the
/// canonical absolute project root to the arch subprocess. When the runner
/// used the relative config dir as `$IRONLINT_ROOT` and cwd, the arch
/// subprocess resolved `--root packages/app` relative to its own cwd
/// (`packages/app`), looked for `packages/app/packages/app`, and failed to
/// build the graph — producing a false block.
#[test]
fn arch_check_blocks_with_nested_relative_config_path() {
    let dir = tempfile::tempdir().unwrap();
    let app_dir = dir.path().join("packages/app");
    fs::create_dir_all(app_dir.join("src/components")).unwrap();
    fs::create_dir_all(app_dir.join("src/data")).unwrap();

    let config_body = "architecture:\n  layers:\n    - name: presentation\n      globs: [\"src/components/**\"]\n    - name: data\n      globs: [\"src/data/**\"]\n  rules:\n    - from: presentation\n      may_import: []\nchecks: {}\n";
    fs::write(app_dir.join(".ironlint.yml"), config_body).unwrap();
    fs::write(app_dir.join("src/data/db.ts"), "export const db = 1;\n").unwrap();
    fs::write(
        app_dir.join("src/components/App.tsx"),
        "import { db } from '../data/db';\nexport { db };\n",
    )
    .unwrap();

    let xdg = tempfile::tempdir().unwrap();
    Command::cargo_bin("ironlint")
        .unwrap()
        .current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .env("PATH", ironlint_path())
        .args(["trust", "--config", "packages/app/.ironlint.yml"])
        .assert()
        .success();

    Command::cargo_bin("ironlint")
        .unwrap()
        .current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .env("PATH", ironlint_path())
        .args(["check", "--config", "packages/app/.ironlint.yml"])
        .assert()
        .code(2)
        .stderr(contains("presentation"));
}

/// Regression for Bug 5: the lowered `__arch__` check must invoke the SAME
/// `ironlint` binary that is running `ironlint check`, not rely on `ironlint`
/// being on `PATH`. If the binary's directory is removed from `PATH`, a bare
/// `ironlint arch check ...` would fail with "command not found" (exit 127 →
/// InternalError) and never report the real architecture violation.
#[test]
fn arch_check_runs_same_binary_when_ironlint_not_on_path() {
    let (dir, config, app) = fixture();
    fs::write(&app, "export function App() { return null; }\n").unwrap();
    let xdg = common::blessed_store(&config);

    // PATH that deliberately excludes the ironlint binary's directory.
    // Keep /usr/bin:/bin so `sh` (the gate shell) is still available.
    let minimal_path = "/usr/bin:/bin";

    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .env("PATH", minimal_path)
        .args([
            "check",
            "--file",
            app.to_str().unwrap(),
            "--content",
            "-",
            "--config",
            ".ironlint.yml",
        ]);
    cmd.write_stdin("import { db } from '../data/db';\nexport { db };\n")
        .assert()
        .code(2)
        .stderr(contains("presentation"));
}

#[test]
fn arch_check_exits_4_when_unblessed() {
    let (dir, _config, app) = fixture();
    fs::write(&app, "export function App() { return null; }\n").unwrap();
    let xdg = tempfile::tempdir().unwrap();

    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .env("PATH", ironlint_path())
        .args([
            "check",
            "--file",
            app.to_str().unwrap(),
            "--content",
            "-",
            "--config",
            ".ironlint.yml",
        ]);
    cmd.write_stdin("import { db } from '../data/db';\nexport { db };\n")
        .assert()
        .code(4);
}
