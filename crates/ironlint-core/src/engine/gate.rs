//! The one execution model: run a gate command, read its exit code.
//!
//! Contract (see spec §3): exit `0` → Pass; `1`–`125` → Block;
//! `126`/`127`/`≥128`/signal/timeout → InternalError (broken-gate fail-open).
//! On Block the combined trimmed stdout+stderr is the message.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

/// The ABI handed to a gate, materialized as process environment + cwd.
pub struct GateEnv<'a> {
    /// Absolute path to the single file under check (`$IRONLINT_FILE`).
    /// `None` when the caller is operating over a set of files with no
    /// single primary target (in which case `$IRONLINT_FILE` is not set).
    pub file: Option<&'a Path>,
    /// All files under check, exported as `$IRONLINT_FILES` (newline-joined,
    /// absolute paths). Per-file checks set this to the singleton `[file]`.
    pub files: &'a [PathBuf],
    /// Project root; also the gate's cwd (`$IRONLINT_ROOT`).
    pub root: &'a Path,
    /// Trigger: `write` | `pre-commit` (`$IRONLINT_EVENT`).
    pub event: &'a str,
    /// Absolute path to an ironlint-materialized temp file holding the proposed
    /// content (`$IRONLINT_TMPFILE`). `Some` only on `write` when the check
    /// references the token; `None` otherwise (var unset).
    pub tmpfile: Option<&'a Path>,
}

#[derive(Debug)]
pub enum InternalReason {
    NotFound,
    NotExecutable,
    Timeout,
    Signal(i32),
    HighExit(i32),
    Spawn(String),
}

impl InternalReason {
    /// Stable string for telemetry / verdict `errors[].reason`.
    pub fn as_str(&self) -> String {
        match self {
            Self::NotFound => "not_found".into(),
            Self::NotExecutable => "not_executable".into(),
            Self::Timeout => "timeout".into(),
            Self::Signal(n) => format!("signal:{n}"),
            Self::HighExit(c) => format!("exit_code:{c}"),
            Self::Spawn(e) => format!("spawn:{e}"),
        }
    }
}

#[derive(Debug)]
pub enum GateOutcome {
    Pass,
    Block { message: String },
    Internal(InternalReason),
}

/// Run one gate against one file. Never panics; spawn failures and timeouts
/// map to `Internal`.
pub fn run_gate(
    run: &str,
    env: &GateEnv,
    content: Option<&[u8]>,
    timeout: Duration,
) -> GateOutcome {
    let files_str = env
        .files
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(run)
        .current_dir(env.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Scrub the child's environment to an explicit allowlist + the IRONLINT_*
    // ABI — a check must not be able to read the agent's full credential set
    // (ANTHROPIC_API_KEY, GITHUB_TOKEN, AWS_*, ...) just because it inherited
    // the parent process environment. env_clear() must run before envs(): the
    // reverse order would wipe out the very vars we just added.
    cmd.env_clear();
    cmd.envs(build_check_env(
        env,
        &files_str,
        &std::env::vars_os().collect::<Vec<_>>(),
    ));
    // Put the child in its own new process group (pgid == its own pid, since
    // it is the group leader). A compound `run` (`cargo check | head`) has
    // `sh` fork-exec the real tool as a group member; on timeout we kill the
    // whole group, not just `sh`, so the real tool never survives orphaned.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return GateOutcome::Internal(InternalReason::Spawn(e.to_string())),
    };

    // Feed proposed content on stdin from a detached thread; a gate that
    // ignores stdin closes the pipe and our write fails with EPIPE, which we
    // intentionally ignore. Without content, close stdin immediately.
    match content {
        Some(bytes) => {
            if let Some(mut stdin) = child.stdin.take() {
                let owned = bytes.to_vec();
                std::thread::spawn(move || {
                    let _ = stdin.write_all(&owned);
                });
            }
        }
        None => drop(child.stdin.take()),
    }

    // Drain stdout/stderr on threads to avoid pipe-buffer deadlock for chatty
    // gates whose output exceeds the pipe capacity before they exit.
    let mut out_pipe = child.stdout.take().expect("stdout piped");
    let mut err_pipe = child.stderr.take().expect("stderr piped");
    let out_handle = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out_pipe.read_to_end(&mut b);
        b
    });
    let err_handle = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err_pipe.read_to_end(&mut b);
        b
    });

    let status = match child.wait_timeout(timeout) {
        Ok(Some(status)) => {
            // Kill the child's process group before joining the drain
            // threads. On the normal path the group leader (`sh`) has
            // already exited and been reaped by `wait_timeout`, but
            // surviving backgrounded descendants (`run: "my-daemon &
            // exit 0"`) still hold the stdout/stderr pipe write-ends.
            // Without this kill, `read_to_end` in the drain threads never
            // sees EOF and the `.join()` calls block until the descendant
            // exits — potentially forever for a non-exiting daemon.
            // Killing the group closes every write-end, guaranteeing the
            // joins return promptly.
            //
            // This intentionally means a check that backgrounds a daemon
            // and exits 0 no longer leaves the daemon running — a gate
            // must not leave residue. A descendant that escaped the group
            // via `setsid`/`setpgid` still evades this kill (same caveat
            // as the timeout branch below), but the common case of a
            // plain `&` background is covered.
            #[cfg(unix)]
            kill_process_group(&child);
            #[cfg(not(unix))]
            kill_process_group(&mut child);
            status
        }
        Ok(None) => {
            #[cfg(unix)]
            kill_process_group(&child);
            #[cfg(not(unix))]
            kill_process_group(&mut child);
            // Reap the direct `sh` child so it doesn't linger as a zombie.
            // It was just SIGKILLed (or `.kill()`ed on non-unix), so this
            // returns promptly.
            let _ = child.wait();
            // Deliberately do NOT join the drain threads here (contrast the
            // normal pass/block path below, which does). `read_to_end` only
            // returns once every write end of the pipe is closed, and
            // `kill_process_group` only reaches processes still in the
            // child's own process group. A descendant that escaped the
            // group (e.g. via `setsid`/`setpgid`) while inheriting our
            // stdout/stderr fd would keep a write end open and block a
            // `.join()` forever — defeating the very timeout this function
            // exists to enforce. Returning here drops `out_handle` /
            // `err_handle` without joining, which detaches the threads:
            // they keep draining in the background until their pipe
            // finally sees EOF (harmless leak, bounded by process
            // lifetime), and their output is never used anyway since
            // `Internal(Timeout)` carries no stdout/stderr message.
            return GateOutcome::Internal(InternalReason::Timeout);
        }
        Err(e) => return GateOutcome::Internal(InternalReason::Spawn(e.to_string())),
    };

    let stdout = String::from_utf8_lossy(&out_handle.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_handle.join().unwrap_or_default()).into_owned();

    classify(status, &stdout, &stderr)
}

/// Vars a check may legitimately need to find and run tools, pulled through
/// from `source` (normally `std::env::vars_os()`). Everything else in the
/// parent environment — API keys, tokens, anything else `sh` inherited — is
/// scrubbed. `LC_*` is a prefix match (locale vars are unbounded: `LC_ALL`,
/// `LC_CTYPE`, `LC_COLLATE`, ...); the rest are exact names.
const ALLOWED_ENV_VARS: &[&str] = &["PATH", "HOME", "LANG", "TZ", "TMPDIR"];

/// Build the child process environment for a check: an allowlisted subset of
/// `source` plus the `IRONLINT_*` ABI. Pure (no process-env access) so it is
/// unit-testable against a synthetic `source` instead of the real, shared,
/// process-global environment.
fn build_check_env(
    env: &GateEnv,
    files_str: &str,
    source: &[(OsString, OsString)],
) -> Vec<(OsString, OsString)> {
    let mut out: Vec<(OsString, OsString)> = source
        .iter()
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            ALLOWED_ENV_VARS.contains(&name.as_ref()) || name.starts_with("LC_")
        })
        .cloned()
        .collect();

    out.push((
        OsString::from("IRONLINT_ROOT"),
        env.root.as_os_str().to_os_string(),
    ));
    out.push((OsString::from("IRONLINT_EVENT"), OsString::from(env.event)));
    out.push((OsString::from("IRONLINT_FILES"), OsString::from(files_str)));
    if let Some(f) = env.file {
        out.push((
            OsString::from("IRONLINT_FILE"),
            f.as_os_str().to_os_string(),
        ));
    }
    if let Some(tf) = env.tmpfile {
        out.push((
            OsString::from("IRONLINT_TMPFILE"),
            tf.as_os_str().to_os_string(),
        ));
    }
    out
}

/// Kill the child's whole process group (it is the group leader — see the
/// `process_group(0)` set at spawn), not just the child itself, so a
/// compound `run` command can't leave its real tool orphaned past a timeout.
#[cfg(unix)]
fn kill_process_group(child: &std::process::Child) {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;
    let pgid = Pid::from_raw(child.id().cast_signed());
    let _ = killpg(pgid, Signal::SIGKILL);
}

#[cfg(not(unix))]
fn kill_process_group(child: &mut std::process::Child) {
    let _ = child.kill();
}

fn classify(status: std::process::ExitStatus, stdout: &str, stderr: &str) -> GateOutcome {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return GateOutcome::Internal(InternalReason::Signal(sig));
        }
    }
    match status.code() {
        Some(0) => GateOutcome::Pass,
        Some(126) => GateOutcome::Internal(InternalReason::NotExecutable),
        Some(127) => GateOutcome::Internal(InternalReason::NotFound),
        Some(c) if c >= 128 => GateOutcome::Internal(InternalReason::HighExit(c)),
        Some(c) if (1..=125).contains(&c) => {
            let message = [stdout.trim(), stderr.trim()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            GateOutcome::Block { message }
        }
        // None == terminated without code on non-unix; treat as internal.
        _ => GateOutcome::Internal(InternalReason::HighExit(-1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn env_for<'a>(file: &'a std::path::Path, root: &'a std::path::Path) -> GateEnv<'a> {
        GateEnv {
            file: Some(file),
            files: &[],
            root,
            event: "write",
            tmpfile: None,
        }
    }

    fn env_with_files<'a>(root: &'a std::path::Path, files: &'a [PathBuf]) -> GateEnv<'a> {
        GateEnv {
            file: None,
            files,
            root,
            event: "write",
            tmpfile: None,
        }
    }

    fn t() -> Duration {
        Duration::from_secs(10)
    }

    #[test]
    fn exit_zero_is_pass() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate("true", &env_for(&f, dir.path()), None, t());
        assert!(matches!(out, GateOutcome::Pass));
    }

    #[test]
    fn exit_one_is_block() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate("exit 1", &env_for(&f, dir.path()), None, t());
        assert!(
            matches!(out, GateOutcome::Block { .. }),
            "exit 1 must block (nonzero = block)"
        );
    }

    #[test]
    fn exit_125_is_block() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate("exit 125", &env_for(&f, dir.path()), None, t());
        assert!(
            matches!(out, GateOutcome::Block { .. }),
            "exit 125 must block (upper edge of 1-125 range)"
        );
    }

    #[test]
    fn exit_two_is_block_with_message() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate(
            "echo problem >&2; exit 2",
            &env_for(&f, dir.path()),
            None,
            t(),
        );
        match out {
            GateOutcome::Block { message } => assert_eq!(message, "problem"),
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn block_with_no_output_is_empty_message() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate("exit 2", &env_for(&f, dir.path()), None, t());
        match out {
            GateOutcome::Block { message } => assert_eq!(message, ""),
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn command_not_found_is_internal() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate(
            "definitely-not-a-real-binary-xyz",
            &env_for(&f, dir.path()),
            None,
            t(),
        );
        assert!(matches!(
            out,
            GateOutcome::Internal(InternalReason::NotFound)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn not_executable_file_is_internal() {
        // Mirrors `command_not_found_is_internal` (the 127 analog): invoking a
        // regular, non-executable file directly (contains a `/`, so the shell
        // execs it rather than searching `$PATH`) makes the shell fail with
        // EACCES and exit 126. `classify()` must map that to
        // `Internal(NotExecutable)`, not `NotFound`.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let notexec = dir.path().join("notexec");
        std::fs::write(&notexec, "#!/bin/sh\necho hi\n").unwrap();
        std::fs::set_permissions(&notexec, std::fs::Permissions::from_mode(0o644)).unwrap();
        let run = format!("\"{}\"", notexec.display());
        let out = run_gate(&run, &env_for(&f, dir.path()), None, t());
        assert!(
            matches!(out, GateOutcome::Internal(InternalReason::NotExecutable)),
            "expected Internal(NotExecutable), got {out:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn signal_death_is_internal_with_signal_reason() {
        // A REAL signal death (the process is killed by SIGTERM, so
        // `ExitStatus::signal()` is `Some(15)`) must classify as
        // `Internal(Signal(15))`. This is distinct from
        // `high_normal_exit_is_internal_with_exit_code_label`, where `exit 137`
        // is a *normal* exit with a high code (no signal involved at all) — the
        // two paths must not be conflated.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate("kill -TERM $$", &env_for(&f, dir.path()), None, t());
        match out {
            GateOutcome::Internal(InternalReason::Signal(n)) => assert_eq!(n, 15),
            other => panic!("expected Internal(Signal(15)), got {other:?}"),
        }
    }

    #[test]
    fn timeout_is_internal() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate(
            "sleep 5",
            &env_for(&f, dir.path()),
            None,
            Duration::from_millis(200),
        );
        assert!(matches!(
            out,
            GateOutcome::Internal(InternalReason::Timeout)
        ));
    }

    #[test]
    fn ironlint_file_env_is_exported() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("target.txt");
        // gate blocks iff $IRONLINT_FILE ends with target.txt
        let out = run_gate(
            "case \"$IRONLINT_FILE\" in *target.txt) exit 2;; *) exit 0;; esac",
            &env_for(&f, dir.path()),
            None,
            t(),
        );
        assert!(matches!(out, GateOutcome::Block { .. }));
    }

    #[test]
    fn high_normal_exit_is_internal_with_exit_code_label() {
        // A normal exit with code >=128 (e.g. `exit 137`) is NOT a signal death —
        // it is a regular exit. It must classify as InternalError, but its reason
        // label must be the raw code ("exit_code:137"), never the misleading
        // "signal:9" (which would imply SIGKILL).
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate("exit 137", &env_for(&f, dir.path()), None, t());
        match out {
            GateOutcome::Internal(reason) => {
                assert_eq!(reason.as_str(), "exit_code:137");
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn proposed_content_arrives_on_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        // block iff stdin contains "FORBIDDEN"
        let out = run_gate(
            "grep -q FORBIDDEN && exit 2 || exit 0",
            &env_for(&f, dir.path()),
            Some(b"line\nFORBIDDEN\n"),
            t(),
        );
        assert!(matches!(out, GateOutcome::Block { .. }));
    }

    #[test]
    fn ironlint_files_is_exported_newline_joined() {
        // Both paths must appear in $IRONLINT_FILES (newline-joined). Gate greps
        // for each; exits 1 (blocks) when both are present, 0 (passes) otherwise.
        let dir = tempfile::tempdir().unwrap();
        let files = vec![PathBuf::from("/p/a.rs"), PathBuf::from("/p/b.rs")];
        let out = run_gate(
            "echo \"$IRONLINT_FILES\" | grep -q 'a.rs' \
             && echo \"$IRONLINT_FILES\" | grep -q 'b.rs' \
             && exit 1 || exit 0",
            &env_with_files(dir.path(), &files),
            None,
            t(),
        );
        assert!(
            matches!(out, GateOutcome::Block { .. }),
            "both files must be visible in $IRONLINT_FILES; got: {out:?}"
        );
    }

    #[test]
    fn tmpfile_env_is_set_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let tmp = dir.path().join("ironlint-tmp-1.txt");
        let env = GateEnv {
            file: Some(&f),
            files: &[],
            root: dir.path(),
            event: "write",
            tmpfile: Some(&tmp),
        };
        // Gate passes iff $IRONLINT_TMPFILE equals the path we passed.
        let run = format!("test \"$IRONLINT_TMPFILE\" = \"{}\"", tmp.display());
        assert!(matches!(run_gate(&run, &env, None, t()), GateOutcome::Pass));
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_whole_process_group() {
        // A compound check (`sleep 30 & ...; wait`) forks a grandchild that
        // `sh` does not wait synchronously for before the timeout fires.
        // Killing only the `sh` child orphans the grandchild. This test
        // proves the grandchild is dead too, not just the immediate child.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let marker = dir.path().join("marker.pid");
        let run = format!("sleep 30 & echo $! > '{}'; wait", marker.display());

        let out = run_gate(
            &run,
            &env_for(&f, dir.path()),
            None,
            Duration::from_millis(500),
        );
        assert!(matches!(
            out,
            GateOutcome::Internal(InternalReason::Timeout)
        ));

        let pid_str = std::fs::read_to_string(&marker)
            .expect("grandchild should have written its pid before the timeout fired");
        let pid: i32 = pid_str.trim().parse().expect("marker should contain a pid");

        // Poll briefly: signal delivery/reaping isn't synchronous with our
        // return, but should resolve well within this window if the whole
        // group was actually killed.
        let mut alive = true;
        for _ in 0..40 {
            match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
                Err(nix::errno::Errno::ESRCH) => {
                    alive = false;
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        assert!(!alive, "grandchild process {pid} survived the timeout kill");
    }

    #[cfg(unix)]
    #[test]
    fn normal_exit_with_backgrounded_descendant_does_not_hang() {
        // A check that backgrounds a long-running descendant and exits 0 hits
        // the NORMAL path (`wait_timeout` returns `Ok(Some(status))`). The
        // backgrounded `sleep 30` inherits the stdout/stderr pipe write-ends,
        // so `read_to_end` in the drain threads never sees EOF and the
        // `.join()` calls hang until the descendant exits (30s, or forever
        // for a non-exiting daemon). The fix: kill the process group before
        // joining the drains on the normal path too, so every pipe write-end
        // is closed and the joins return promptly.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let start = std::time::Instant::now();
        let out = run_gate(
            "sleep 30 & exit 0",
            &env_for(&f, dir.path()),
            None,
            Duration::from_secs(1),
        );
        let elapsed = start.elapsed();
        assert!(
            matches!(out, GateOutcome::Pass),
            "expected Pass, got {out:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "normal exit with backgrounded descendant took {elapsed:?} — should be well under 5s"
        );
    }

    #[cfg(unix)]
    #[test]
    fn timeout_detaches_when_descendant_setsids_away() {
        // Regression pin for the f65d68c timeout-branch drain fix: a
        // descendant that `setsid`s out of the child's process group while
        // inheriting stdout survives `kill_process_group` (it's no longer in
        // the group). The timeout branch must NOT join the drain threads in
        // that case — joining would block forever on the inherited pipe.
        // Instead it detaches and returns `Internal(Timeout)` promptly.
        // The outer `sh` sleeps 30 (past the timeout), and a setsid'd
        // grandchild also holds the stdout pipe, so this exercises the
        // timeout branch's detach-don't-join path.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let start = std::time::Instant::now();
        let out = run_gate(
            "setsid sh -c 'exec sleep 30' & sleep 30",
            &env_for(&f, dir.path()),
            None,
            Duration::from_millis(500),
        );
        let elapsed = start.elapsed();
        assert!(
            matches!(out, GateOutcome::Internal(InternalReason::Timeout)),
            "expected Internal(Timeout), got {out:?}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "timeout branch with setsid'd descendant took {elapsed:?} — should detach and return promptly"
        );
    }

    #[test]
    fn build_check_env_scrubs_secrets_and_keeps_allowlist() {
        // Synthetic source env — never touches real process env (process-global
        // mutation via std::env::set_var would be flaky across parallel tests).
        let source = vec![
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (OsString::from("HOME"), OsString::from("/home/x")),
            (OsString::from("LC_ALL"), OsString::from("C")),
            (OsString::from("SECRET_TOKEN"), OsString::from("shhh")),
            (
                OsString::from("ANTHROPIC_API_KEY"),
                OsString::from("sk-xxx"),
            ),
        ];
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let tmp = dir.path().join("ironlint-tmp-1.txt");
        let files = vec![f.clone()];
        let env = GateEnv {
            file: Some(&f),
            files: &files,
            root: dir.path(),
            event: "write",
            tmpfile: Some(&tmp),
        };
        let files_str = "irrelevant-files-str";

        let result = build_check_env(&env, files_str, &source);
        let get = |k: &str| {
            result
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
        };

        // Allowlisted vars pass through with their values.
        assert_eq!(get("PATH"), Some(OsString::from("/usr/bin:/bin")));
        assert_eq!(get("HOME"), Some(OsString::from("/home/x")));
        assert_eq!(get("LC_ALL"), Some(OsString::from("C")));

        // Non-allowlisted vars — including secrets — are scrubbed.
        assert!(
            get("SECRET_TOKEN").is_none(),
            "secret must not leak through"
        );
        assert!(
            get("ANTHROPIC_API_KEY").is_none(),
            "API key must not leak through"
        );

        // The IRONLINT_* ABI is still fully present.
        assert_eq!(
            get("IRONLINT_ROOT"),
            Some(dir.path().as_os_str().to_os_string())
        );
        assert_eq!(get("IRONLINT_EVENT"), Some(OsString::from("write")));
        assert_eq!(get("IRONLINT_FILES"), Some(OsString::from(files_str)));
        assert_eq!(get("IRONLINT_FILE"), Some(f.as_os_str().to_os_string()));
        assert_eq!(
            get("IRONLINT_TMPFILE"),
            Some(tmp.as_os_str().to_os_string())
        );
    }

    #[test]
    fn build_check_env_omits_file_and_tmpfile_when_absent() {
        let source: Vec<(OsString, OsString)> = vec![];
        let dir = tempfile::tempdir().unwrap();
        let env = GateEnv {
            file: None,
            files: &[],
            root: dir.path(),
            event: "pre-commit",
            tmpfile: None,
        };

        let result = build_check_env(&env, "", &source);
        let has = |k: &str| result.iter().any(|(name, _)| name == k);

        assert!(!has("IRONLINT_FILE"), "no file → var must be unset");
        assert!(!has("IRONLINT_TMPFILE"), "no tmpfile → var must be unset");
        assert!(has("IRONLINT_ROOT"));
        assert!(has("IRONLINT_EVENT"));
        assert!(has("IRONLINT_FILES"));
    }

    #[test]
    fn tmpfile_env_is_unset_when_none() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let env = GateEnv {
            file: Some(&f),
            files: &[],
            root: dir.path(),
            event: "write",
            tmpfile: None,
        };
        // With the var unset, `test -n` on it is false → exit 1 → Block. Pass means it WAS set (bug).
        assert!(matches!(
            run_gate("test -n \"$IRONLINT_TMPFILE\"", &env, None, t()),
            GateOutcome::Block { .. }
        ));
    }
}
