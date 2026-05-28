//! `hector check --diff` against a POSIX `diff -u`-style patch (with a
//! `\t<timestamp>` on the header path) must strip the timestamp so scope
//! matching succeeds and the configured rules actually run.

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let cfg = dir.join(".hector.yml");
    fs::write(&cfg, body).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    cfg
}

#[test]
fn cli_check_diff_with_posix_timestamp_blocks() {
    let tmp = tempdir().unwrap();

    let cfg_body = "\
schema_version: 2
rules:
  no-todo:
    description: \"no todo\"
    engine: script
    scope: [\"*.py\"]
    severity: error
    script: \"grep -q TODO {file} && exit 1 || exit 0\"
";
    let cfg = write_trusted(tmp.path(), cfg_body);

    // Create the target file (script rule cwd is the config dir).
    let target = tmp.path().join("myfile.py");
    fs::write(&target, "# TODO: ship it\n").unwrap();

    // Synthesize a POSIX-style patch with timestamps.
    let patch = tmp.path().join("t.patch");
    fs::write(
        &patch,
        "--- a/myfile.py\t2026-05-24 14:30:00 +0000\n\
         +++ b/myfile.py\t2026-05-24 14:30:00 +0000\n\
         @@ -1,1 +1,2 @@\n\
          # was here\n\
         +# TODO: ship it\n",
    )
    .unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["check", "--diff"])
        .arg(&patch)
        .arg("--config")
        .arg(&cfg)
        .arg("--format")
        .arg("json")
        .current_dir(tmp.path())
        .output()
        .expect("run hector");

    // The rule blocks → exit 2.
    assert_eq!(
        out.status.code(),
        Some(2),
        "POSIX-timestamp patch must exit 2; stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
