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

fn build_mounts(
    policy: &std::path::Path,
    fixture: &std::path::Path,
    cases: &std::path::Path,
    drive: &std::path::Path,
    run_dir: &std::path::Path,
    hector_bin: &std::path::Path,
) -> [String; 6] {
    [
        format!("{}:/work/policy/.hector.yml:ro", policy.display()),
        format!("{}:/work/fixture:ro", fixture.display()),
        format!("{}:/work/cases:ro", cases.display()),
        format!("{}:/work/drive.sh:ro", drive.display()),
        format!("{}:/work/runs:rw", run_dir.display()),
        format!("{}:/usr/local/bin/hector:ro", hector_bin.display()),
    ]
}

pub fn run_case(adapter: &str, case: &str) -> anyhow::Result<RunResult> {
    let root = workspace_root()?;
    let e2e = root.join("tests/e2e");

    let case_path = e2e.join("cases").join(format!("{case}.json"));
    let case_text = std::fs::read_to_string(&case_path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", case_path.display()))?;
    let case_json: serde_json::Value = serde_json::from_str(&case_text)?;
    let target_file = case_json
        .get("target_file")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("case {case}: missing string field `target_file`"))?
        .to_string();

    let run_dir = e2e.join(adapter).join("runs").join(case);
    if run_dir.exists() {
        std::fs::remove_dir_all(&run_dir)?;
    }
    std::fs::create_dir_all(&run_dir)?;

    let env_file = e2e.join(".env.e2e");
    let policy = e2e.join("policy/.hector.yml");
    let fixture = e2e.join("fixture");
    let cases = e2e.join("cases");
    let drive = e2e.join(adapter).join("drive.sh");
    let hector_bin = root.join("target/release/hector");
    let image = format!("hector-e2e-{adapter}:latest");

    let mounts = build_mounts(&policy, &fixture, &cases, &drive, &run_dir, &hector_bin);

    let mut cmd = Command::new("docker");
    cmd.arg("run").arg("--rm");
    cmd.arg("--env-file").arg(&env_file);
    for m in &mounts {
        cmd.args(["-v", m]);
    }
    cmd.arg(&image);
    cmd.arg(format!("--case={case}"));

    let output = cmd.output()?;
    let exit_code = output.status.code().unwrap_or(-1);

    RunResult::from_run_dir(&run_dir, exit_code, &target_file)
}
