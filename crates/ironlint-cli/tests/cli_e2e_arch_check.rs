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
