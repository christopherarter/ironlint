//! The one execution model: run a gate command, read its exit code.
//!
//! Contract (see spec §3): exit `2` → Block; `126`/`127`/`≥128`/timeout →
//! InternalError; everything else → Pass. On Block the combined trimmed
//! stdout+stderr is the message.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

/// The ABI handed to a gate, materialized as process environment + cwd.
pub struct GateEnv<'a> {
    /// Absolute path to the file under check (`$HECTOR_FILE`).
    pub file: &'a Path,
    /// Project root; also the gate's cwd (`$HECTOR_ROOT`).
    pub root: &'a Path,
    /// Trigger: `edit` | `write` | `pre-commit` | `manual` (`$HECTOR_EVENT`).
    pub event: &'a str,
}

#[derive(Debug)]
pub enum InternalReason {
    NotFound,
    NotExecutable,
    Timeout,
    Signal(i32),
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
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(run)
        .current_dir(env.root)
        .env("HECTOR_FILE", env.file)
        .env("HECTOR_ROOT", env.root)
        .env("HECTOR_EVENT", env.event)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
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
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            return GateOutcome::Internal(InternalReason::Timeout);
        }
        Err(e) => return GateOutcome::Internal(InternalReason::Spawn(e.to_string())),
    };

    let stdout = String::from_utf8_lossy(&out_handle.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_handle.join().unwrap_or_default()).into_owned();

    classify(status, &stdout, &stderr)
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
        Some(2) => {
            let message = [stdout.trim(), stderr.trim()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            GateOutcome::Block { message }
        }
        Some(126) => GateOutcome::Internal(InternalReason::NotExecutable),
        Some(127) => GateOutcome::Internal(InternalReason::NotFound),
        Some(c) if c >= 128 => GateOutcome::Internal(InternalReason::Signal(c - 128)),
        _ => GateOutcome::Pass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn env_for<'a>(file: &'a std::path::Path, root: &'a std::path::Path) -> GateEnv<'a> {
        GateEnv {
            file,
            root,
            event: "manual",
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
    fn exit_one_is_pass() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate("exit 1", &env_for(&f, dir.path()), None, t());
        assert!(
            matches!(out, GateOutcome::Pass),
            "exit 1 must be Pass (opt-in blocking)"
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
    fn hector_file_env_is_exported() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("target.txt");
        // gate blocks iff $HECTOR_FILE ends with target.txt
        let out = run_gate(
            "case \"$HECTOR_FILE\" in *target.txt) exit 2;; *) exit 0;; esac",
            &env_for(&f, dir.path()),
            None,
            t(),
        );
        assert!(matches!(out, GateOutcome::Block { .. }));
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
}
