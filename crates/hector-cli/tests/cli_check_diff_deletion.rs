use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn cli_check_diff_pure_deletion_passes_clean() {
    let tmp = tempdir().unwrap();
    let cfg_body = "schema_version: 2\nrules:\n  r:\n    description: x\n    \
                    engine: script\n    scope: [\"**/*.rs\"]\n    severity: error\n    \
                    script: \"false\"\n";
    let cfg = tmp.path().join(".hector.yml");
    fs::write(&cfg, cfg_body).unwrap();
    let signed = hector_core::trust::write_trust_block(&fs::read_to_string(&cfg).unwrap()).unwrap();
    fs::write(&cfg, signed).unwrap();

    let patch = tmp.path().join("d.patch");
    fs::write(
        &patch,
        "--- a/gone.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-fn a() {}\n-fn b() {}\n",
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
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(0),
        "pure-deletion diff must exit 0; stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}
