use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn session_record_creates_session_file() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "content\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "session",
            "record",
            "--dir",
            dir.path().to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--diff",
            "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new",
        ])
        .assert()
        .success();
    let path = dir.path().join(".hector/session.json");
    assert!(path.exists());
    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("a.txt"));
    assert!(body.contains("+new"));
}

#[test]
fn session_record_appends_when_file_exists() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "content\n").unwrap();
    // First record
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "session",
            "record",
            "--dir",
            dir.path().to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--diff",
            "diff1",
        ])
        .assert()
        .success();
    // Second record
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "session",
            "record",
            "--dir",
            dir.path().to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--diff",
            "diff2",
        ])
        .assert()
        .success();
    let path = dir.path().join(".hector/session.json");
    let body = fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let edits = json["edits"].as_array().unwrap();
    assert_eq!(edits.len(), 2);
}

#[test]
fn session_record_fails_on_corrupt_session_file() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "content\n").unwrap();
    // Write a corrupt session.json.
    let session_path = dir.path().join(".hector");
    fs::create_dir_all(&session_path).unwrap();
    fs::write(session_path.join("session.json"), "not valid json").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "session",
            "record",
            "--dir",
            dir.path().to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--diff",
            "diff",
        ])
        .assert()
        .failure();
}
