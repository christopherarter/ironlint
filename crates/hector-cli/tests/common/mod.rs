//! Shared helpers for CLI integration tests.
use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

/// Bless `config` in a fresh, isolated trust store and return the `TempDir`
/// that backs it. Keep the returned guard alive for the test, and set
/// `XDG_CONFIG_HOME` to `guard.path()` on every `hector` invocation that runs
/// `check`, so they all read the same blessed store.
#[must_use]
pub fn blessed_store(config: &Path) -> TempDir {
    let xdg = tempfile::tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(config)
        .assert()
        .success();
    xdg
}
