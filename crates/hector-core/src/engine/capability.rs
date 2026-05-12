use crate::config::Capabilities;
use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

/// Hard wall-clock cap on a single script-rule subprocess.
///
/// A runaway shell (infinite loop, hung `tail -f`, accidental `sleep 30`)
/// previously blocked the entire `check` invocation because `Command::output()`
/// reads to EOF. The timeout fires here, the child is killed, and the runner
/// returns a synthetic `ExecOutcome` so callers can render a verdict like any
/// other failure. See P1-12 in `docs/2026-05-12-bug-audit.md`.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Per-stream cap on captured output (1 MiB). A noisy linter that floods
/// stdout/stderr would otherwise grow `String::from_utf8_lossy` allocations
/// until the host OOMs.
const MAX_OUTPUT: usize = 1 << 20;

/// Exit code reported when the timeout fires. Matches GNU coreutils' `timeout(1)`
/// convention so existing tooling and operators recognise the meaning.
const TIMEOUT_EXIT_CODE: i32 = 124;

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
            return spawn_with_timeout(cmd, cwd, env);
        }
        // Probe succeeded — the parent is now inside the requested namespace,
        // and the child will inherit it. Do NOT unshare again in pre_exec.
    }

    spawn_with_timeout(cmd, cwd, env)
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
    spawn_with_timeout(cmd, cwd, env)
}

/// Spawn `sh -c <cmd>` in `cwd` with `env` overrides, enforcing both a
/// wall-clock timeout and a per-stream output cap.
///
/// No namespace work happens here — on Linux, the parent has already done
/// the unshare (or fallen back to best-effort) before reaching this point.
/// Used by macOS and the Linux post-unshare path; centralising the spawn
/// keeps the timeout + bounded-read invariant in exactly one place.
fn spawn_with_timeout(cmd: &str, cwd: &Path, env: &[(&str, &str)]) -> Result<ExecOutcome> {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        command.env(k, v);
    }
    let mut child = command.spawn().context("spawning script subprocess")?;

    let Some(status) = child
        .wait_timeout(TIMEOUT)
        .context("waiting for subprocess")?
    else {
        // Timeout fired. Kill, reap, and synthesise a hector-prefixed
        // stderr so consumers can distinguish a timeout from a genuine
        // non-zero exit. Exit code 124 matches GNU `timeout(1)`.
        let _ = child.kill();
        let _ = child.wait();
        return Ok(ExecOutcome {
            stdout: String::new(),
            stderr: format!("hector: script killed after {TIMEOUT:?} timeout"),
            exit_code: TIMEOUT_EXIT_CODE,
        });
    };

    // Bounded reads: drop anything beyond MAX_OUTPUT per stream. Lossy-utf8
    // decoding here would require buffering the raw bytes first; using
    // `read_to_string` on a `Read::take(N)` gives us the cap for free, and
    // any invalid UTF-8 is reported as an I/O error which we swallow (the
    // stream is best-effort diagnostic output, not load-bearing).
    let mut stdout = String::new();
    if let Some(out) = child.stdout.take() {
        let _ = out.take(MAX_OUTPUT as u64).read_to_string(&mut stdout);
    }
    let mut stderr = String::new();
    if let Some(err) = child.stderr.take() {
        let _ = err.take(MAX_OUTPUT as u64).read_to_string(&mut stderr);
    }

    Ok(ExecOutcome {
        stdout,
        stderr,
        exit_code: status.code().unwrap_or(-1),
    })
}
