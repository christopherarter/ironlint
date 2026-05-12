use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn migrate_renames_bully_to_hector() {
    let dir = tempdir().unwrap();
    let bully = dir.path().join(".bully.yml");
    fs::write(
        &bully,
        "schema_version: 1\nrules:\n  r:\n    description: x\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
    ).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["migrate", "--dir", dir.path().to_str().unwrap()])
        .assert()
        .success();
    let hector = dir.path().join(".hector.yml");
    assert!(hector.exists(), ".hector.yml written");
    assert!(bully.exists(), ".bully.yml preserved by default");
    let content = fs::read_to_string(&hector).unwrap();
    assert!(content.contains("schema_version: 2"));
}

#[test]
fn migrate_moves_state_dir() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join(".bully.yml"),
        "schema_version: 1\nrules: {}\n",
    )
    .unwrap();
    fs::create_dir(dir.path().join(".bully")).unwrap();
    fs::write(dir.path().join(".bully/log.jsonl"), "{\"x\":1}\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["migrate", "--dir", dir.path().to_str().unwrap()])
        .assert()
        .success();
    assert!(dir.path().join(".hector/log.jsonl").exists());
}
