mod common;

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn validate_from_subdir_finds_parent_config() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join(".ironlint.yml");
    fs::write(
        &cfg,
        "checks:\n  py:\n    files: [\"*.py\"]\n    run: \"true\"\n",
    )
    .unwrap();
    // Create a subdir and run from there with cwd set to it.
    let subdir = tmp.path().join("src/nested");
    fs::create_dir_all(&subdir).unwrap();
    let xdg = common::blessed_store(&cfg);

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["validate"])
        .current_dir(&subdir)
        .env("XDG_CONFIG_HOME", xdg.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ok: 1 check(s)"),
        "validate should walk up to the parent .ironlint.yml; saw stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn no_config_anywhere_gives_init_pointer() {
    let tmp = tempdir().unwrap();
    // No .ironlint.yml anywhere. Run validate from a subdir.
    let subdir = tmp.path().join("deep");
    fs::create_dir_all(&subdir).unwrap();

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["validate"])
        .current_dir(&subdir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ironlint init"),
        "missing-config error must point at the fix; saw stderr:\n{stderr}"
    );
    assert_eq!(out.status.code(), Some(1));
}
