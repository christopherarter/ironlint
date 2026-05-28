//! `hector check --diff` against a mismatched diff (where `--- a/<other>`
//! precedes `+++ b/<target>`) must produce a verdict for the correct file
//! only. `build_single_file_diff` verifies the `--- a/` header path matches
//! the target before including it, so a foreign header is dropped.

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

/// The diff has `--- a/src/a.rs` followed by `+++ b/src/b.rs` — a mismatched
/// header pair. Only `src/b.rs` is modified; `src/a.rs` must not appear in
/// the verdict as a violation source.
#[test]
fn cli_check_mismatched_minus_header_does_not_produce_phantom_file() {
    let tmp = tempdir().unwrap();

    // Script rule fires whenever the file contains "BANNED".
    let cfg_body = "\
schema_version: 2
rules:
  no-banned:
    description: \"no BANNED token\"
    engine: script
    scope: [\"*.rs\"]
    severity: error
    script: \"grep -q BANNED {file} && exit 1 || exit 0\"
";
    let cfg = write_trusted(tmp.path(), cfg_body);

    // `src/b.rs` is the file being changed — it contains the banned token.
    // `src/a.rs` does NOT exist on disk; it would fail the rule if run.
    let b_file = tmp.path().join("src").join("b.rs");
    fs::create_dir_all(b_file.parent().unwrap()).unwrap();
    fs::write(&b_file, "// BANNED\n").unwrap();

    // Mismatched diff: `--- a/src/a.rs` but `+++ b/src/b.rs`. The foreign
    // `--- a/` header must be dropped so only `src/b.rs` is processed.
    let patch = tmp.path().join("t.patch");
    fs::write(
        &patch,
        "--- a/src/a.rs\n\
         +++ b/src/b.rs\n\
         @@ -1,1 +1,1 @@\n\
         -// old\n\
         +// BANNED\n",
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

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // The rule fires on src/b.rs (which exists and has BANNED) → exit 2.
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2; stdout={stdout:?} stderr={stderr:?}"
    );

    // The verdict must name src/b.rs, not src/a.rs.
    assert!(
        stdout.contains("src/b.rs"),
        "verdict must reference src/b.rs; stdout={stdout:?}"
    );
    assert!(
        !stdout.contains("src/a.rs"),
        "verdict must not reference the phantom src/a.rs; stdout={stdout:?}"
    );
}
