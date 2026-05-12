use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn init_scaffolds_for_rust_project() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"foo\"\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["init", "--dir", dir.path().to_str().unwrap()])
        .assert()
        .success();
    let cfg = fs::read_to_string(dir.path().join(".hector.yml")).unwrap();
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
