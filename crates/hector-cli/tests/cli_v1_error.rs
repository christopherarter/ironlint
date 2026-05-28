use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

/// A fresh v1 config (no trust block yet) must emit a "run `hector migrate`"
/// hint before falling through to the generic "trust block missing" error.
/// v1 detection runs before `trust::verify` so the user isn't steered toward
/// `hector trust` and a v1-body / v2-trust-block hybrid.
#[test]
fn v1_config_without_trust_emits_migrate_hint() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(
        &cfg,
        "schema_version: 1\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    )
    .unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "clean\n").unwrap();

    let output = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
        ])
        .assert()
        // Internal/config error → exit 1.
        .code(1)
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8_lossy(&output);
    assert!(
        stderr.contains("migrate"),
        "expected `migrate` hint in stderr, got: {stderr}"
    );
}

/// The same hint must surface for a v1 config that has acquired a trust
/// block, because the v1 body itself is the problem. Detecting v1 before
/// trust verify also avoids a confusing "fingerprint mismatch" error here.
#[test]
fn v1_config_with_trust_block_still_emits_migrate_hint_on_load() {
    use hector_core::runner::HectorEngine;
    use hector_core::trust::write_trust_block;
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    let body = "schema_version: 1\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n";
    let trusted = write_trust_block(body).unwrap();
    fs::write(&cfg, &trusted).unwrap();

    let err = match HectorEngine::load(&cfg) {
        Ok(_) => panic!("v1 must not load"),
        Err(e) => e,
    };
    let msg = format!("{err:#}");
    assert!(
        msg.contains("migrate"),
        "expected `migrate` hint in core load error, got: {msg}"
    );
}
