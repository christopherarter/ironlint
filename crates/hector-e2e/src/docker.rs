//! Docker shell-outs.

use crate::env::workspace_root;
use crate::result::RunResult;
use std::process::Command;

const BASE_TAG: &str = "hector-e2e-base:latest";

/// Build the shared base image and the per-adapter image. Idempotent.
pub fn build_image(adapter: &str) -> anyhow::Result<()> {
    let root = workspace_root()?;
    let base_dir = root.join("tests/e2e/base");
    let adapter_dir = root.join("tests/e2e").join(adapter);

    if !adapter_dir.join("Dockerfile").exists() {
        anyhow::bail!(
            "no Dockerfile at {} — unknown adapter {adapter:?}",
            adapter_dir.display(),
        );
    }

    let status = Command::new("docker")
        .args(["build", "-t", BASE_TAG, "."])
        .current_dir(&base_dir)
        .status()?;
    if !status.success() {
        anyhow::bail!("docker build (base) failed with status {status}");
    }

    let adapter_tag = format!("hector-e2e-{adapter}:latest");
    let status = Command::new("docker")
        .args(["build", "-t", &adapter_tag, "."])
        .current_dir(&adapter_dir)
        .status()?;
    if !status.success() {
        anyhow::bail!("docker build ({adapter}) failed with status {status}");
    }
    Ok(())
}

pub fn run_case(_adapter: &str, _case: &str) -> anyhow::Result<RunResult> {
    anyhow::bail!("not yet implemented")
}
