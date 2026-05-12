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
fn session_record_concurrent_writers_do_not_clobber() {
    // P2-1 regression: previously `record` did a non-atomic read-modify-write,
    // so two concurrent writers would each load the same baseline and one
    // would clobber the other. With flock around load+save, all 16 records
    // must end up in the final session.json.
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let file_path = dir_path.join("a.txt");
    fs::write(&file_path, "content\n").unwrap();

    let n: usize = 16;
    let mut handles = Vec::with_capacity(n);
    for i in 0..n {
        let dir_str = dir_path.to_str().unwrap().to_string();
        let file_str = file_path.to_str().unwrap().to_string();
        // Distinct diff per thread so we can verify every record landed.
        let diff = format!("DIFF-{i:02}");
        handles.push(std::thread::spawn(move || {
            Command::cargo_bin("hector")
                .unwrap()
                .args([
                    "session",
                    "record",
                    "--dir",
                    &dir_str,
                    "--file",
                    &file_str,
                    "--diff",
                    &diff,
                    "--session-id",
                    "race",
                ])
                .assert()
                .success();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let path = dir_path.join(".hector/session.json");
    let body = fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let edits = json["edits"].as_array().unwrap();
    assert_eq!(
        edits.len(),
        n,
        "expected {n} edits to survive concurrent writers, got {}",
        edits.len()
    );
    // Sanity: every distinct DIFF-NN value is present.
    let mut diffs: Vec<String> = edits
        .iter()
        .map(|e| e["diff"].as_str().unwrap().to_string())
        .collect();
    diffs.sort();
    let mut want: Vec<String> = (0..n).map(|i| format!("DIFF-{i:02}")).collect();
    want.sort();
    assert_eq!(diffs, want);
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
