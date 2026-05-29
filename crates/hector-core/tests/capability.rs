use hector_core::config::{Capabilities, WritesPolicy};
use hector_core::engine::capability::run_with_capabilities;
// `run_with_capabilities_env` is exercised only by the Linux-gated clone-path
// test below; importing it unconditionally trips `-D unused-imports` on macOS.
#[cfg(target_os = "linux")]
use hector_core::engine::capability::run_with_capabilities_env;
use hector_core::engine::capability::run_with_capabilities_stdin;
use std::path::PathBuf;

#[test]
fn allows_command_with_no_network_and_no_writes() {
    let caps = Capabilities {
        network: false,
        writes: WritesPolicy::None,
    };
    let cwd = PathBuf::from(".");
    let outcome = run_with_capabilities("echo hello", &cwd, &caps).expect("run");
    assert!(outcome.stdout.contains("hello"));
    assert_eq!(outcome.exit_code, 0);
}

#[test]
fn rejects_unknown_writes_policy_via_parser() {
    // parser-level guarantee: serde rejects unknown variants for WritesPolicy.
    let yaml = "writes: bogus\n";
    let result: std::result::Result<Capabilities, _> = serde_yaml::from_str(yaml);
    assert!(result.is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn capability_run_succeeds_for_unprivileged_user() {
    // Regression: an unprivileged caller must still be able to run script
    // rules. unshare(CLONE_NEWNET) without CLONE_NEWUSER returns EPERM for
    // unprivileged callers, so the parent probes unshare in advance and, on
    // EPERM, falls back to best-effort with a one-time stderr warning.
    let caps = Capabilities {
        network: false,
        writes: WritesPolicy::CwdOnly,
    };
    let out = run_with_capabilities("echo ok", std::path::Path::new("/tmp"), &caps)
        .expect("must run without privilege");
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("ok"));
}

#[test]
fn capability_run_kills_runaway_command() {
    // Regression: a runaway script rule like `sleep 30` must not wedge the
    // whole `check` invocation. The capability runner spawns the child with
    // piped stdio and enforces a wall-clock timeout via `wait_timeout`,
    // returning a hector-prefixed stderr and exit code 124 (matches GNU
    // `timeout`).
    //
    // Use unrestricted caps so the Linux unshare path doesn't take its
    // fallback branch — the timeout logic lives in the spawn helper that
    // both paths share, so this exercises macOS and the Linux post-unshare
    // path uniformly without requiring privilege.
    let start = std::time::Instant::now();
    let caps = Capabilities {
        network: true,
        writes: WritesPolicy::Unrestricted,
    };
    let out = run_with_capabilities("sleep 30", std::path::Path::new("/tmp"), &caps)
        .expect("runner must return Ok even when child is killed");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(10),
        "runaway must be killed within timeout; took {:?}",
        start.elapsed()
    );
    assert_ne!(
        out.exit_code, 0,
        "killed process must report non-zero exit; got {}",
        out.exit_code
    );
    assert!(
        out.stderr.contains("killed")
            || out.stderr.contains("timeout")
            || out.stderr.contains("hector"),
        "stderr should mention the kill reason; was: {:?}",
        out.stderr
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_network_disabled_blocks_network_attempts() {
    let caps = Capabilities {
        network: false,
        writes: WritesPolicy::None,
    };
    let cwd = PathBuf::from(".");
    // Try to resolve a hostname; should fail in the netns
    let outcome = run_with_capabilities(
        "getent hosts example.com >/dev/null 2>&1 && echo NET || echo NONET",
        &cwd,
        &caps,
    )
    .expect("run");
    assert!(
        outcome.stdout.contains("NONET"),
        "expected NONET in netns, got: {}",
        outcome.stdout
    );
}

#[test]
fn captures_large_output_without_deadlocking() {
    // A script that writes more than the OS pipe buffer (~64 KiB on Linux)
    // before exiting must not deadlock. Before the concurrent-drain fix the
    // child blocks on write(2), never exits, and trips the 5s timeout with
    // empty stdout and exit code 124. `network: true` keeps Linux on the
    // shared spawn_with_timeout fast path (no clone), exercising the path
    // every platform shares.
    let caps = Capabilities {
        network: true,
        writes: WritesPolicy::Unrestricted,
    };
    let start = std::time::Instant::now();
    // `yes x` emits "x\n" forever; `head -n 200000` caps the pipeline at
    // ~400 KiB and exits 0 (sh's status is the last pipeline stage).
    let out = run_with_capabilities(
        "yes x | head -n 200000",
        std::path::Path::new("/tmp"),
        &caps,
    )
    .expect("runner returns Ok");
    assert_eq!(
        out.exit_code, 0,
        "must exit cleanly, not time out (124); stderr was: {:?}",
        out.stderr
    );
    assert!(
        out.stdout.len() > 64 * 1024,
        "must capture more than one pipe buffer of stdout; got {} bytes",
        out.stdout.len()
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "must not hit the 5s timeout; took {:?}",
        start.elapsed()
    );
}

#[test]
fn timeout_does_not_hang_when_a_backgrounded_process_holds_the_pipe() {
    // Regression: on timeout the runner must NOT block joining the output
    // readers. `sleep 30 & sleep 30` keeps sh alive past the 5s deadline (the
    // foreground sleep) while a backgrounded sleep inherits and holds the
    // stdout pipe write-end. Joining the reader would wait for that grandchild
    // to exit (~30s), defeating the timeout; detaching returns promptly.
    let caps = Capabilities {
        network: true,
        writes: WritesPolicy::Unrestricted,
    };
    let start = std::time::Instant::now();
    let out = run_with_capabilities("sleep 30 & sleep 30", std::path::Path::new("/tmp"), &caps)
        .expect("runner returns Ok");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(8),
        "timeout path must not block on a backgrounded pipe holder; took {:?}",
        start.elapsed()
    );
    assert_eq!(out.exit_code, 124, "must report the timeout exit code");
    assert!(
        out.stderr.contains("killed") || out.stderr.contains("timeout"),
        "stderr should mention the timeout kill; was: {:?}",
        out.stderr
    );
}

#[cfg(target_os = "linux")]
#[test]
fn captures_large_output_on_clone_path() {
    // wait_for_child (the network:false clone path) must also drain
    // concurrently. On unprivileged CI, clone(2) EPERM falls back to
    // spawn_with_timeout, which is also fixed — so this holds either way.
    let caps = Capabilities {
        network: false,
        writes: WritesPolicy::None,
    };
    let start = std::time::Instant::now();
    let out = run_with_capabilities(
        "yes x | head -n 200000",
        std::path::Path::new("/tmp"),
        &caps,
    )
    .expect("runner returns Ok");
    assert_eq!(
        out.exit_code, 0,
        "must exit cleanly; stderr: {:?}",
        out.stderr
    );
    assert!(
        out.stdout.len() > 64 * 1024,
        "must capture >64 KiB; got {}",
        out.stdout.len()
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "took {:?}",
        start.elapsed()
    );
}

#[test]
fn pipes_stdin_content_to_command() {
    // `cat` echoes whatever it reads from stdin. With the new stdin parameter,
    // the proposed content must reach the child.
    let caps = Capabilities {
        network: true, // keep on the fast path (no clone) so this runs anywhere
        writes: WritesPolicy::None,
    };
    let out = run_with_capabilities_stdin(
        "cat",
        std::path::Path::new("."),
        &caps,
        &[],
        Some(b"hello stdin"),
    )
    .expect("run");
    assert_eq!(out.stdout, "hello stdin");
    assert_eq!(out.exit_code, 0);
}

#[test]
fn ignored_stdin_does_not_hang_or_error() {
    // A command that never reads stdin must still complete cleanly. The writer
    // swallows BrokenPipe; a payload larger than the OS pipe buffer (64 KiB)
    // proves the writer thread can't deadlock the wait.
    let caps = Capabilities {
        network: true,
        writes: WritesPolicy::None,
    };
    let big = "x".repeat(256 * 1024);
    let out = run_with_capabilities_stdin(
        "echo done",
        std::path::Path::new("."),
        &caps,
        &[],
        Some(big.as_bytes()),
    )
    .expect("run");
    assert_eq!(out.stdout.trim(), "done");
    assert_eq!(out.exit_code, 0);
}

#[test]
fn none_stdin_preserves_legacy_behavior() {
    // No stdin supplied → child inherits parent fd 0 as before; a plain command
    // still runs.
    let caps = Capabilities {
        network: true,
        writes: WritesPolicy::None,
    };
    let out =
        run_with_capabilities_stdin("echo legacy", std::path::Path::new("."), &caps, &[], None)
            .expect("run");
    assert_eq!(out.stdout.trim(), "legacy");
    assert_eq!(out.exit_code, 0);
}

#[cfg(target_os = "linux")]
#[test]
fn clone_child_receives_injected_env_and_inherits_path() {
    // network:false routes through the clone path on Linux. The injected var
    // must reach the child, AND the parent's PATH must survive — execve
    // replaces the environment wholesale, so the runner must forward the full
    // parent env plus the overrides. (On unprivileged CI, clone(2) returns
    // EPERM and falls back to spawn_with_timeout, which also forwards env, so
    // this holds on both paths.)
    let caps = Capabilities {
        network: false,
        writes: WritesPolicy::None,
    };
    let out = run_with_capabilities_env(
        "printf '%s\\n' \"$HECTOR_TEST_VAR\"; command -v sh >/dev/null && echo PATH_OK",
        std::path::Path::new("/tmp"),
        &caps,
        &[("HECTOR_TEST_VAR", "injected-value")],
    )
    .expect("run");
    assert!(
        out.stdout.contains("injected-value"),
        "injected env var must reach the child; stdout: {:?}",
        out.stdout
    );
    assert!(
        out.stdout.contains("PATH_OK"),
        "inherited PATH must survive execve; stdout: {:?}",
        out.stdout
    );
}
