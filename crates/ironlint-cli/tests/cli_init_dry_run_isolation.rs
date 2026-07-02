//! Regression guard for `ironlint init --dry-run`: a dry-run must NOT write
//! `.ironlint.yml` and must NOT bless anything in the trust store.
//!
//! `--dry-run` is documented as "Print intended changes without writing." The
//! scaffold phase (`scaffold_config`) writes the config AND calls
//! `trust::bless`, so it must be skipped entirely on the dry-run path.
//! Previously `scaffold_config` was called unconditionally, mutating the
//! security-critical trust store even in a dry-run.

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

/// `init --dry-run` must touch neither the project dir nor the trust store.
#[test]
fn init_dry_run_writes_no_config_and_no_trust_store() {
    let dir = tempdir().unwrap();
    // Isolate the trust store in a throwaway XDG_CONFIG_HOME so the real
    // user store is never touched, and so we can assert it stays absent.
    let xdg = tempdir().unwrap();

    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "init",
            "--dir",
            dir.path().to_str().unwrap(),
            "--no-hook",
            "--dry-run",
        ])
        .assert()
        .success();

    // No config file must be scaffolded into the project dir.
    let cfg = dir.path().join(".ironlint.yml");
    assert!(
        !cfg.exists(),
        "dry-run must NOT write the config file, but found {}",
        cfg.display()
    );

    // The trust store file must NOT have been created anywhere under XDG.
    let trust_store = xdg.path().join("ironlint").join("trust.json");
    assert!(
        !trust_store.exists(),
        "dry-run must NOT bless the config (trust store was created at {})",
        trust_store.display()
    );
}

/// `init --dry-run` against a project that ALREADY has `.ironlint.yml` must
/// mirror the real skip logic: report "already present (would skip)" instead
/// of "would scaffold and trust", leave the existing config byte-for-byte
/// unchanged, and still create no trust store. (A dry-run preview that claims
/// it "would scaffold and trust" when a real run would do neither is a lie.)
#[test]
fn init_dry_run_with_existing_config_reports_skip() {
    let dir = tempdir().unwrap();
    let xdg = tempdir().unwrap();

    // A pre-existing config with distinctive content we can byte-compare.
    let cfg = dir.path().join(".ironlint.yml");
    let original = "checks:\n  my-own:\n    files: [\"*\"]\n    run: \"true\"\n";
    fs::write(&cfg, original).unwrap();

    let assert = Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "init",
            "--dir",
            dir.path().to_str().unwrap(),
            "--no-hook",
            "--dry-run",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("already present (would skip)"),
        "dry-run over an existing config must report the skip, got: {stdout:?}"
    );
    assert!(
        !stdout.contains("would scaffold and trust"),
        "dry-run must NOT claim it would scaffold/trust an existing config, got: {stdout:?}"
    );

    // The pre-existing config must be byte-for-byte unchanged.
    let after = fs::read_to_string(&cfg).unwrap();
    assert_eq!(
        after, original,
        "dry-run must not modify the existing config"
    );

    // No trust store may be created anywhere under XDG.
    let trust_store = xdg.path().join("ironlint").join("trust.json");
    assert!(
        !trust_store.exists(),
        "dry-run must NOT bless the config (trust store created at {})",
        trust_store.display()
    );
}

/// The bare `init --dry-run` (no `--no-hook`, default auto-detect) still must
/// not have written the config file by the time it finishes, even though the
/// hook phase runs and prints a plan. This covers the path where `run` falls
/// through to `run_hook_phase` after skipping the scaffold.
#[test]
fn init_dry_run_without_no_hook_still_writes_no_config() {
    let dir = tempdir().unwrap();
    let xdg = tempdir().unwrap();

    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["init", "--dir", dir.path().to_str().unwrap(), "--dry-run"])
        .assert()
        .success();

    let cfg = dir.path().join(".ironlint.yml");
    assert!(
        !cfg.exists(),
        "dry-run (auto-detect hook phase) must NOT write the config file, but found {}",
        cfg.display()
    );

    // Belt-and-suspenders: confirm the dir is otherwise empty (no stray files).
    let entries: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(
        entries.is_empty(),
        "dry-run must leave the project dir empty, found: {:?}",
        entries.iter().map(|e| e.file_name()).collect::<Vec<_>>()
    );
}
