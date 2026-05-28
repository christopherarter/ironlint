use hector_core::config::{Capabilities, WritesPolicy};
use hector_core::engine::capability::run_with_capabilities;
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
