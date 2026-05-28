//! Preflight check for the host environment.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Returns true if Docker, the API-key file, and `target/release/hector`
/// are all present. Writes one line to stderr per missing dep and returns
/// false otherwise.
#[must_use]
pub fn require_e2e_env() -> bool {
    let Ok(root) = workspace_root() else {
        eprintln!("skipping: CARGO_MANIFEST_DIR not set");
        return false;
    };

    let docker_ok = docker_present();
    let env_ok = root.join("tests/e2e/.env.e2e").exists();
    let bin_ok = root.join("target/release/hector").exists();

    if !docker_ok {
        eprintln!("skipping: `docker` not on PATH");
    }
    if !env_ok {
        eprintln!(
            "skipping: tests/e2e/.env.e2e missing (copy .env.e2e.example and fill ANTHROPIC_API_KEY)",
        );
    }
    if !bin_ok {
        eprintln!("skipping: target/release/hector missing (run `cargo build --release`)");
    }
    docker_ok && env_ok && bin_ok
}

pub(crate) fn workspace_root() -> anyhow::Result<PathBuf> {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| anyhow::anyhow!("CARGO_MANIFEST_DIR not set"))?;
    Ok(Path::new(&crate_dir)
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("crate dir has no grandparent"))?
        .to_path_buf())
}

fn docker_present() -> bool {
    Command::new("docker")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_is_two_up_from_crate() {
        // CARGO_MANIFEST_DIR for hector-e2e is crates/hector-e2e/.
        let root = workspace_root().expect("CARGO_MANIFEST_DIR set in cargo test");
        assert!(root.join("Cargo.toml").exists());
        assert!(root.join("crates").is_dir());
        // tests/e2e/ may not exist on first run — root resolution still valid
        let _ = root.join("tests").join("e2e").is_dir();
    }

    #[test]
    fn docker_present_returns_bool_without_panic() {
        // We don't assert the value — docker may or may not be on PATH in CI —
        // but the function must not panic or hang.
        let _ = docker_present();
    }

    #[test]
    fn require_e2e_env_returns_false_when_env_file_absent() {
        // In the test environment, tests/e2e/.env.e2e does not exist, so
        // require_e2e_env must return false. This exercises all three guard
        // branches: docker_ok (whatever it is), env_ok (false), bin_ok (may
        // be false in a dev-only build), and the final `&&` short-circuit.
        // The function writes to stderr — that's fine for a test.
        let result = require_e2e_env();
        // .env.e2e is absent; the overall result must be false.
        assert!(!result, "require_e2e_env must be false without .env.e2e");
    }
}
