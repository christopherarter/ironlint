//! `hector baseline refresh` re-hashes every entry to match current file
//! content.

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

fn trusted_config(body: &str) -> String {
    hector_core::trust::write_trust_block(body).unwrap()
}

#[test]
fn refresh_updates_checksums_to_current_content() {
    // Use an AST rule so violations carry line numbers — refresh only
    // re-hashes entries with an explicit `line`. A `script:` rule would
    // emit `line: None` and refresh would correctly pass-through, which
    // doesn't exercise the checksum-update path we're testing here.
    let dir = tempdir().unwrap();
    let root = dir.path();
    let cfg_body = "schema_version: 2\nrules:\n  no-unwrap:\n    description: x\n    engine: ast\n    language: rust\n    scope: [\"**/*.rs\"]\n    severity: warning\n    pattern: $E.unwrap()\n";
    let cfg = root.join(".hector.yml");
    fs::write(&cfg, trusted_config(cfg_body)).unwrap();

    // Seed a file with a violation and record a baseline.
    let file = root.join("a.rs");
    fs::write(&file, "fn main() {\n    let x = foo.unwrap();\n}\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "baseline",
            "--config",
            cfg.to_str().unwrap(),
            "--scan",
            "*.rs",
        ])
        .current_dir(root)
        .assert()
        .success();

    let baseline_path = root.join(".hector/baseline.json");
    let before = fs::read_to_string(&baseline_path).unwrap();
    // V2 shape: `entries: { ... }`, and the recorded entry must carry a
    // real checksum (the `add_with_content` path captured the line text).
    assert!(
        before.contains("\"entries\""),
        "v2 shape expected on first record: {before}"
    );
    assert!(
        !before.contains(": null"),
        "first record must capture a line-content checksum, not null: {before}"
    );

    // Edit the violating line — same rule still fires, but the content
    // hash changes. Refresh should rewrite the stored checksum to match.
    fs::write(&file, "fn main() {\n    let y = bar.unwrap();\n}\n").unwrap();

    // Refresh.
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["baseline", "refresh", "--config", cfg.to_str().unwrap()])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "refresh failed (code={:?}): stdout={} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let after = fs::read_to_string(&baseline_path).unwrap();
    assert_ne!(
        before, after,
        "refresh must update at least one stored checksum"
    );
}

#[test]
fn refresh_with_no_baseline_succeeds_silently() {
    let dir = tempdir().unwrap();
    let cfg_body = "schema_version: 2\nrules:\n  noop:\n    description: x\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n";
    let cfg = dir.path().join(".hector.yml");
    fs::write(&cfg, trusted_config(cfg_body)).unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .args(["baseline", "refresh", "--config", cfg.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();
}

#[test]
fn refresh_upgrades_v1_baseline_to_v2() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let cfg_body = "schema_version: 2\nrules:\n  noop:\n    description: x\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n";
    let cfg = root.join(".hector.yml");
    fs::write(&cfg, trusted_config(cfg_body)).unwrap();

    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    let file = src.join("lib.txt");
    fs::write(&file, "fn main() {}\nTODO: ship E1\n").unwrap();

    // Plant a v1-format baseline by hand. The inner fingerprint is itself
    // a JSON-encoded 3-tuple string — embed it via `serde_json::json!` so
    // we don't fight Rust's string escaping.
    fs::create_dir_all(root.join(".hector")).unwrap();
    let fp = r#"["todo-marker","src/lib.txt",2]"#;
    let v1 = serde_json::json!({ "fingerprints": [fp] }).to_string();
    fs::write(root.join(".hector/baseline.json"), v1).unwrap();

    // Refresh.
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["baseline", "refresh", "--config", cfg.to_str().unwrap()])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "refresh failed (code={:?}): stdout={} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    // The deprecation warning must appear on stderr once.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("legacy v1 format"),
        "expected v1 deprecation warning on stderr; got: {stderr}"
    );

    // The on-disk file is now in v2 shape and contains a real checksum.
    let after = fs::read_to_string(root.join(".hector/baseline.json")).unwrap();
    assert!(
        after.contains("\"entries\""),
        "post-refresh file must be v2: {after}"
    );
    assert!(
        !after.contains("\"fingerprints\""),
        "v1 key should be gone: {after}"
    );
}
