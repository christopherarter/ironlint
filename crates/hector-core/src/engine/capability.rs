use crate::config::Capabilities;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

pub struct ExecOutcome {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Run a shell command under the requested capability constraints.
/// Returns Ok(outcome) if the process completed (even non-zero exit).
pub fn run_with_capabilities(cmd: &str, cwd: &Path, caps: &Capabilities) -> Result<ExecOutcome> {
    run_with_capabilities_env(cmd, cwd, caps, &[])
}

/// Same as [`run_with_capabilities`], with an extra env-injection slice.
///
/// Each `(name, value)` pair is applied to the child process environment
/// before spawning. Used by the script engine to pass attacker-controlled
/// values (like file paths) to the shell via env vars rather than splicing
/// them into the command text.
pub fn run_with_capabilities_env(
    cmd: &str,
    cwd: &Path,
    caps: &Capabilities,
    env: &[(&str, &str)],
) -> Result<ExecOutcome> {
    #[cfg(target_os = "linux")]
    {
        run_linux(cmd, cwd, caps, env)
    }
    #[cfg(not(target_os = "linux"))]
    {
        run_best_effort_macos(cmd, cwd, caps, env)
    }
}

#[cfg(target_os = "linux")]
fn run_linux(
    cmd: &str,
    cwd: &Path,
    caps: &Capabilities,
    env: &[(&str, &str)],
) -> Result<ExecOutcome> {
    use nix::sched::CloneFlags;
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);

    // Compute the desired isolation flags. Writes-policy enforcement is a
    // documented no-op in 0.1 (see docs/security.md and CLAUDE.md), so we
    // only request CLONE_NEWNET here — claiming CLONE_NEWNS without
    // remounting anything would be theatre.
    let mut flags = CloneFlags::empty();
    if !caps.network {
        flags.insert(CloneFlags::CLONE_NEWNET);
    }

    // Probe whether unshare with the requested flags would succeed. If not
    // (typically EPERM for unprivileged users without CLONE_NEWUSER), fall
    // back to a best-effort spawn with a one-time stderr warning. Before
    // this probe, the unshare lived inside `pre_exec`, where its failure
    // surfaced as a confusing "running command" error and turned every
    // script rule into an `__internal` Block verdict (P0-8).
    //
    // NOTE: a successful unshare here mutates the *parent* process's
    // namespaces — it's a one-shot, process-wide side effect. That's fine
    // for the `hector` CLI (one process per invocation). Tests that share
    // a process should not assume the network is reachable afterwards if
    // any earlier test in the suite has unshared CLONE_NEWNET.
    if !flags.is_empty() {
        let probe = unsafe { libc::unshare(flags.bits()) };
        if probe != 0 {
            let err = std::io::Error::last_os_error();
            if !WARNED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "hector: capability sandbox unavailable for unprivileged user ({err}); \
                     running command without isolation. See docs/security.md."
                );
            }
            return spawn_unrestricted(cmd, cwd, env);
        }
        // Probe succeeded — the parent is now inside the requested namespace,
        // and the child will inherit it. Do NOT unshare again in pre_exec.
    }

    spawn_unrestricted(cmd, cwd, env)
}

#[cfg(not(target_os = "linux"))]
fn run_best_effort_macos(
    cmd: &str,
    cwd: &Path,
    caps: &Capabilities,
    env: &[(&str, &str)],
) -> Result<ExecOutcome> {
    use crate::config::WritesPolicy;
    // macOS: caps are advisory; log the limitation, run normally.
    if !caps.network || !matches!(caps.writes, WritesPolicy::Unrestricted) {
        eprintln!(
            "hector: capability enforcement is best-effort on this platform (see docs/security.md); running command unrestricted"
        );
    }
    spawn_unrestricted(cmd, cwd, env)
}

/// Spawn `sh -c <cmd>` in `cwd` with `env` overrides — no namespace work.
/// Used on macOS, and on Linux either after a successful parent-side
/// unshare or when falling back from an EPERM probe.
fn spawn_unrestricted(cmd: &str, cwd: &Path, env: &[(&str, &str)]) -> Result<ExecOutcome> {
    let mut child = Command::new("sh");
    child.arg("-c").arg(cmd).current_dir(cwd);
    for (k, v) in env {
        child.env(k, v);
    }
    let output = child.output().context("running command")?;
    Ok(ExecOutcome {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}
