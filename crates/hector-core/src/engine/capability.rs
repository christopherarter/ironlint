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
/// other failure. See P1-12 in `docs/audits/2026-05-12-bug-audit.md`.
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

/// Linux entry point. Computes the desired isolation flags and routes to
/// `clone(2)` when any are requested; otherwise falls through to the
/// shared `spawn_with_timeout` fast path used by macOS and the
/// no-isolation case.
///
/// **B6 invariant.** The parent process never `unshare`s. Namespace flags
/// are applied to the cloned child only, so the next rule in the same
/// `hector` invocation sees a clean parent state. Before B6, a single
/// `network: false` rule mutated the parent and silently broke every
/// subsequent `network: true` rule.
#[cfg(target_os = "linux")]
fn run_linux(
    cmd: &str,
    cwd: &Path,
    caps: &Capabilities,
    env: &[(&str, &str)],
) -> Result<ExecOutcome> {
    use nix::sched::CloneFlags;

    // Compute the desired isolation flags. Writes-policy enforcement is a
    // documented no-op in 0.1 (see docs/security.md and CLAUDE.md), so we
    // only request CLONE_NEWNET here — claiming CLONE_NEWNS without
    // remounting anything would be theatre.
    let mut flags = CloneFlags::empty();
    if !caps.network {
        flags.insert(CloneFlags::CLONE_NEWNET);
    }

    if flags.is_empty() {
        // Fast path: no isolation requested, use the cheap `std::process`
        // spawn shared with macOS. Keeps the common case (`network: true`,
        // the default) free of `clone(2)` and child-stack allocation.
        return spawn_with_timeout(cmd, cwd, env);
    }

    spawn_clone_with_timeout(cmd, cwd, env, flags)
}

/// Spawn `sh -c <cmd>` via `clone(2)` with the requested namespace flags
/// applied to the child only, enforcing the same wall-clock timeout and
/// per-stream output cap as `spawn_with_timeout`.
///
/// On a privilege-related failure (`clone(2)` returning `EPERM`, which
/// is what unprivileged hosts without `CLONE_NEWUSER` get), this falls
/// back to the unrestricted `spawn_with_timeout` path with a one-time
/// stderr warning — matching the existing P0-8 behaviour. Preserving the
/// fallback is the audit's stated "no UX regression for unprivileged
/// users" constraint.
#[cfg(target_os = "linux")]
fn spawn_clone_with_timeout(
    cmd: &str,
    cwd: &Path,
    env: &[(&str, &str)],
    flags: nix::sched::CloneFlags,
) -> Result<ExecOutcome> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);

    let (child_pid, stdout_r, stderr_r) = match spawn_clone(cmd, cwd, env, flags) {
        Ok(triple) => triple,
        Err(err) => {
            // Most likely EPERM from `clone(2)` without privilege —
            // identical UX to the pre-B6 fallback. We can't probe ahead
            // of time without mutating the parent (the whole point of B6
            // is that we don't), so a real spawn attempt is the probe.
            if !WARNED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "hector: capability sandbox unavailable for unprivileged user ({err}); \
                     running command without isolation. See docs/security.md."
                );
            }
            return spawn_with_timeout(cmd, cwd, env);
        }
    };
    wait_for_child(child_pid, stdout_r, stderr_r)
}

/// Allocate the child's stack and `clone(2)` it. Returns the child pid
/// and the read ends of stdout/stderr pipes; the write ends are kept
/// alive only inside the child closure and dropped here in the parent.
///
/// Layout note: `nix::sched::clone` is `unsafe fn` because the caller is
/// responsible for (a) keeping the stack alive for the lifetime of the
/// child and (b) not corrupting the parent's heap from inside the child
/// closure. We satisfy (a) by storing the stack in a leaked `Box<[u8]>`
/// — the child runs until `execv` replaces its address space, and the
/// stack memory is reclaimed by the kernel when the child exits. We
/// satisfy (b) by having the closure only call `execv` and writes that
/// don't touch shared heap state.
#[cfg(target_os = "linux")]
fn spawn_clone(
    cmd: &str,
    cwd: &Path,
    env: &[(&str, &str)],
    flags: nix::sched::CloneFlags,
) -> Result<(nix::unistd::Pid, std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    use nix::fcntl::OFlag;
    use nix::unistd::pipe2;
    use std::os::fd::AsRawFd;

    // `O_CLOEXEC` so that the parent-end fds get closed automatically if
    // the child ever forks again before `execv` (defense in depth — our
    // child closure only does `execv`).
    let (stdout_r, stdout_w) = pipe2(OFlag::O_CLOEXEC).context("pipe2 for child stdout")?;
    let (stderr_r, stderr_w) = pipe2(OFlag::O_CLOEXEC).context("pipe2 for child stderr")?;

    // 64 KiB stack: nix's clone recommends ≥16 KiB; we pick 64 KiB to leave
    // headroom for `sh`'s startup (which runs inside this stack until
    // `execv` swaps in its own).
    let mut stack: Box<[u8]> = vec![0u8; 64 * 1024].into_boxed_slice();

    // Capture the bits we need inside the child by value. Closure must
    // not borrow anything from the parent's stack frame — after
    // `clone(2)` the parent and child run independently, and any pointer
    // into the parent's frame would dangle in the child.
    let cmd_string = cmd.to_string();
    let cwd_path = cwd.to_path_buf();
    let env_vec: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let stdout_w_raw = stdout_w.as_raw_fd();
    let stderr_w_raw = stderr_w.as_raw_fd();

    let child_fn: nix::sched::CloneCb<'_> = Box::new(move || -> isize {
        child_main(stdout_w_raw, stderr_w_raw, &cwd_path, &env_vec, &cmd_string)
    });

    // SAFETY: `nix::sched::clone` is `unsafe fn` because the caller must
    // (1) keep `stack` alive for the lifetime of the child and (2) not
    // share parent heap state with the child after the call. We satisfy
    // (1) by binding `stack` in this function's scope — the parent does
    // not free it until after `waitpid` succeeds in the caller, and even
    // if the parent panics the OS will reap the child via SIGCHLD before
    // the stack is dropped. We satisfy (2) by having the closure only
    // call `child_main`, which performs syscalls (`dup2`, `chdir`,
    // `setenv`, `execv`) that do not deallocate the parent's heap.
    // SAFETY-MIRI: `clone(2)` is opaque to miri; the per-child
    // namespace invariant is verified empirically by
    // `tests/capability_per_child.rs` on Linux instead.
    #[allow(unsafe_code)]
    let pid = unsafe { nix::sched::clone(child_fn, &mut stack, flags, Some(libc::SIGCHLD)) }
        .context("clone(2) for capability-sandboxed child")?;

    // Stack must outlive the child. Leak the box so it's never dropped —
    // memory is reclaimed by the kernel on process exit. The leak is
    // O(64 KiB) per script-rule invocation; acceptable given hector runs
    // tens to hundreds of rules per check, not millions.
    Box::leak(stack);

    // Close the parent's copy of the write ends. If we don't, reads from
    // the read ends will block forever — they only return EOF when every
    // writer has closed.
    drop(stdout_w);
    drop(stderr_w);

    Ok((pid, stdout_r, stderr_r))
}

/// Body of the cloned child. Runs in the child's address space until
/// `execv` replaces it. Returns an `isize` exit code; on `execv` success
/// this function does not return.
///
/// Conventions:
/// - exit 126: `chdir` or `setenv` failed before exec
/// - exit 127: `execv` failed (command not found / not executable) —
///   matches POSIX shell convention for "command not found"
#[cfg(target_os = "linux")]
fn child_main(
    stdout_w_raw: std::os::fd::RawFd,
    stderr_w_raw: std::os::fd::RawFd,
    cwd: &Path,
    env: &[(String, String)],
    cmd: &str,
) -> isize {
    use nix::unistd::{dup2, execv};
    use std::ffi::CString;

    // Redirect stdout/stderr to the pipe write-ends. `nix::unistd::dup2`
    // is a safe wrapper around `dup2(2)`.
    if dup2(stdout_w_raw, 1).is_err() || dup2(stderr_w_raw, 2).is_err() {
        return 126;
    }

    if std::env::set_current_dir(cwd).is_err() {
        return 126;
    }

    // SAFETY: `std::env::set_var` is `unsafe fn` in recent Rust because
    // mutating env mid-program is racy with other threads reading env.
    // Here we are in the freshly-cloned child immediately after
    // `clone(2)`: only one thread exists (the kernel does not clone
    // sibling threads), so no race is possible. The next syscall after
    // this loop is `execv`, which replaces the entire address space.
    #[allow(unsafe_code)]
    for (k, v) in env {
        unsafe {
            std::env::set_var(k, v);
        }
    }

    // Build `execv` arguments. CStrings live on the child's stack frame
    // and are preserved across the `execv` call (the kernel copies the
    // strings into the new image's argv before swapping address spaces).
    let Ok(sh) = CString::new("/bin/sh") else {
        return 126;
    };
    let Ok(arg0) = CString::new("sh") else {
        return 126;
    };
    let Ok(argc) = CString::new("-c") else {
        return 126;
    };
    let Ok(argv) = CString::new(cmd) else {
        return 126;
    };

    // `nix::unistd::execv` is safe; it does not return on success.
    let _ = execv(&sh, &[&arg0, &argc, &argv]);

    // `execv` only returns on error — anything past this is "command not
    // found / not executable", matching POSIX shell convention.
    127
}

/// Wait for the cloned child to exit, polling `waitpid(WNOHANG)` until
/// the deadline elapses. On timeout, sends `SIGKILL`, reaps the child,
/// and returns a synthesised timeout outcome (exit 124, matches
/// `timeout(1)`). On normal exit, drains stdout/stderr from the pipe
/// read ends with the `MAX_OUTPUT` cap and returns the captured output.
///
/// Note: matches `spawn_with_timeout`'s wait-then-read semantics. If a
/// child writes more than the kernel pipe buffer (~64 KiB) before
/// exiting it will block on `write(2)` and trip the timeout — same
/// failure mode as the existing path, tracked separately if it ever
/// becomes load-bearing.
#[cfg(target_os = "linux")]
fn wait_for_child(
    pid: nix::unistd::Pid,
    stdout_r: std::os::fd::OwnedFd,
    stderr_r: std::os::fd::OwnedFd,
) -> Result<ExecOutcome> {
    use nix::sys::signal::{kill, Signal};
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};

    let deadline = std::time::Instant::now() + TIMEOUT;
    let poll_interval = std::time::Duration::from_millis(10);

    let exit_status = loop {
        match waitpid(pid, Some(WaitPidFlag::WNOHANG)).context("waitpid on cloned child")? {
            WaitStatus::StillAlive => {
                if std::time::Instant::now() >= deadline {
                    // Timeout fired. Kill and reap. `kill` failures are
                    // ignored — typically ESRCH if the child raced us
                    // and exited between the WNOHANG check and the
                    // signal, in which case we just need to drain via
                    // blocking waitpid.
                    let _ = kill(pid, Signal::SIGKILL);
                    let _ = waitpid(pid, None);
                    return Ok(ExecOutcome {
                        stdout: String::new(),
                        stderr: format!("hector: script killed after {TIMEOUT:?} timeout"),
                        exit_code: TIMEOUT_EXIT_CODE,
                    });
                }
                std::thread::sleep(poll_interval);
            }
            other => break other,
        }
    };

    // Child has exited. Drain the pipes with the per-stream cap and
    // synthesise the outcome.
    let (stdout, stderr) = read_pipes_bounded(stdout_r, stderr_r);
    Ok(ExecOutcome {
        stdout,
        stderr,
        exit_code: exit_status_to_code(exit_status),
    })
}

/// Drain stdout and stderr read-ends, each capped at `MAX_OUTPUT`. Wraps
/// the `OwnedFd`s in `std::fs::File` to get `Read`; the file closes its
/// fd on drop, matching the `OwnedFd` lifetime.
#[cfg(target_os = "linux")]
fn read_pipes_bounded(
    stdout_r: std::os::fd::OwnedFd,
    stderr_r: std::os::fd::OwnedFd,
) -> (String, String) {
    let mut stdout = String::new();
    let _ = std::fs::File::from(stdout_r)
        .take(MAX_OUTPUT as u64)
        .read_to_string(&mut stdout);
    let mut stderr = String::new();
    let _ = std::fs::File::from(stderr_r)
        .take(MAX_OUTPUT as u64)
        .read_to_string(&mut stderr);
    (stdout, stderr)
}

/// Map a `WaitStatus` to a POSIX-style exit code. Normal exits return
/// the program's status; signal terminations are reported as `128 +
/// signum`, mirroring shell conventions so consumers can recognise
/// "killed by SIGKILL = 137" without re-encoding the convention here.
#[cfg(target_os = "linux")]
fn exit_status_to_code(status: nix::sys::wait::WaitStatus) -> i32 {
    use nix::sys::wait::WaitStatus;
    match status {
        WaitStatus::Exited(_, code) => code,
        WaitStatus::Signaled(_, sig, _) => 128 + sig as i32,
        _ => -1,
    }
}

#[cfg(not(target_os = "linux"))]
fn run_best_effort_macos(
    cmd: &str,
    cwd: &Path,
    _caps: &Capabilities,
    env: &[(&str, &str)],
) -> Result<ExecOutcome> {
    // R7 (2026-05-23): no eprintln here. The platform-best-effort
    // story is surfaced by `hector doctor` (see `platform_capability_status`
    // and `commands::doctor::check_capabilities`) rather than by every
    // `check` invocation. Pre-R7 we deduped per-process via a static
    // AtomicBool, but the Claude Code adapter hook spawns ~3 hector
    // processes per edit so the warning still leaked to users.
    spawn_with_timeout(cmd, cwd, env)
}

/// Platform-level capability story, exposed for the `hector doctor`
/// surface (and any other diagnostic consumer).
///
/// Returns `None` on platforms that enforce the requested capability
/// constraints (today: Linux via namespaces). Returns `Some(message)` on
/// platforms where the enforcement is best-effort. The message is a
/// short human-readable sentence safe to embed in a doctor row's
/// `detail` field; it does not end with a period so the doctor renderer
/// can chain text after it.
///
/// Kept here (not in `commands::doctor`) so platform knowledge lives
/// next to the runner that depends on it — if a future platform gains
/// real enforcement, the one place to update is this file.
#[must_use]
pub fn platform_capability_status() -> Option<&'static str> {
    #[cfg(target_os = "linux")]
    {
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        Some(
            "capability enforcement is best-effort on this platform; script rules run unrestricted",
        )
    }
}

/// Spawn `sh -c <cmd>` in `cwd` with `env` overrides, enforcing both a
/// wall-clock timeout and a per-stream output cap.
///
/// No namespace work happens here — on Linux, this is the no-isolation
/// fast path (and the EPERM fallback for `clone(2)`); on macOS it's the
/// only path. Centralising the spawn keeps the timeout + bounded-read
/// invariant in exactly one place across both targets.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn platform_capability_status_is_none_on_linux() {
        // Linux enforces network isolation via CLONE_NEWNET. The doctor
        // row collapses to a pass with no message when there's nothing
        // to advise about.
        assert!(platform_capability_status().is_none());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn platform_capability_status_is_some_on_macos() {
        // macOS (and any non-Linux target) cannot enforce the requested
        // capability constraints today. Doctor surfaces this via a
        // `capabilities` row whose `detail` includes the returned
        // message verbatim. Pre-R7 the same message was eprintln!'d
        // from every script-rule invocation; consolidating it here
        // keeps routine `check` runs quiet on stderr.
        let msg = platform_capability_status().expect("non-linux platform reports a message");
        assert!(
            msg.contains("best-effort"),
            "message should describe the limitation: {msg}"
        );
        assert!(
            !msg.ends_with('.'),
            "message should not end with a period (the doctor row appends context): {msg}"
        );
    }
}
