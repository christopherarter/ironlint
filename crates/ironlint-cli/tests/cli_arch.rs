use assert_cmd::Command;
use std::fs;

fn fixture(allows_data: bool) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/components")).unwrap();
    fs::create_dir_all(dir.path().join("src/data")).unwrap();
    fs::create_dir_all(dir.path().join(".ironlint")).unwrap();
    fs::write(dir.path().join("src/data/db.ts"), "export const db = 1;\n").unwrap();
    fs::write(
        dir.path().join("src/components/App.tsx"),
        "import { db } from '../data/db';\nexport { db };\n",
    )
    .unwrap();
    let may_import = if allows_data { "[data]" } else { "[]" };
    let layers = dir.path().join(".ironlint/arch.yml");
    fs::write(
        &layers,
        format!(
            "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\nrules:\n  - from: presentation\n    may_import: {may_import}\n"
        ),
    )
    .unwrap();
    (dir, layers)
}

#[test]
fn arch_check_blocks_on_forbidden_edge() {
    let (dir, layers) = fixture(false);
    Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "arch",
            "check",
            "--root",
            dir.path().to_str().unwrap(),
            "--layers",
            layers.to_str().unwrap(),
        ])
        .assert()
        .code(2);
}

#[test]
fn arch_check_passes_when_clean() {
    let (dir, layers) = fixture(true);
    Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "arch",
            "check",
            "--root",
            dir.path().to_str().unwrap(),
            "--layers",
            layers.to_str().unwrap(),
        ])
        .assert()
        .code(0);
}

#[test]
fn arch_check_write_blocks_on_forbidden_edge() {
    let (dir, layers) = fixture(false);
    Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "arch",
            "check",
            "--event",
            "write",
            "--file",
            "src/components/App.tsx",
            "--root",
            dir.path().to_str().unwrap(),
            "--layers",
            layers.to_str().unwrap(),
        ])
        .write_stdin("import { db } from '../data/db';\nexport { db };\n")
        .assert()
        .code(2);
}

#[test]
fn arch_check_maps_internal_error_to_exit_3() {
    let (_dir, layers) = fixture(false);
    Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "arch",
            "check",
            "--root",
            "/nonexistent/ironlint/arch/root",
            "--layers",
            layers.to_str().unwrap(),
        ])
        .assert()
        .code(3);
}

#[test]
fn arch_check_maps_missing_layers_to_exit_3() {
    let (dir, _layers) = fixture(false);
    Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "arch",
            "check",
            "--root",
            dir.path().to_str().unwrap(),
            "--layers",
            dir.path().join(".ironlint/missing.yml").to_str().unwrap(),
        ])
        .assert()
        .code(3);
}

#[test]
fn arch_graph_supports_dot_and_json() {
    let (dir, layers) = fixture(false);
    let dot = Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "arch",
            "graph",
            "--dot",
            "--root",
            dir.path().to_str().unwrap(),
            "--layers",
            layers.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(dot.status.success());
    assert!(String::from_utf8_lossy(&dot.stdout).contains("digraph"));

    let json = Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "arch",
            "graph",
            "--json",
            "--root",
            dir.path().to_str().unwrap(),
            "--layers",
            layers.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(json.status.success());
    assert!(String::from_utf8_lossy(&json.stdout)
        .trim_start()
        .starts_with('{'));

    Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "arch",
            "graph",
            "--dot",
            "--json",
            "--root",
            dir.path().to_str().unwrap(),
            "--layers",
            layers.to_str().unwrap(),
        ])
        .assert()
        .failure();
}

#[test]
fn arch_graph_defaults_to_dot() {
    let (dir, layers) = fixture(false);
    let output = Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "arch",
            "graph",
            "--root",
            dir.path().to_str().unwrap(),
            "--layers",
            layers.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("digraph"));
}

#[test]
fn arch_why_errors_with_exit_3_when_layers_missing() {
    let (dir, _layers) = fixture(false);
    Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "arch",
            "why",
            "src/components/App.tsx",
            "--root",
            dir.path().to_str().unwrap(),
            "--layers",
            dir.path().join(".ironlint/missing.yml").to_str().unwrap(),
        ])
        .assert()
        .code(3);
}

#[test]
fn arch_why_accepts_a_root_relative_path() {
    let (dir, layers) = fixture(false);
    let output = Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "arch",
            "why",
            "src/components/App.tsx",
            "--root",
            dir.path().to_str().unwrap(),
            "--layers",
            layers.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("presentation"));
}
