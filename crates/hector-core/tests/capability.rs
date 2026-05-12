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
    // Regression for P0-8: pre-fix, unshare(CLONE_NEWNET) without CLONE_NEWUSER
    // returned EPERM for unprivileged callers, which made `Command::output()`
    // error and every script rule produced an `__internal` Block verdict.
    // Post-fix: the parent probes unshare in advance; on EPERM it falls back
    // to best-effort with a one-time stderr warning, so the command still runs.
    let caps = Capabilities {
        network: false,
        writes: WritesPolicy::CwdOnly,
    };
    let out = run_with_capabilities("echo ok", std::path::Path::new("/tmp"), &caps)
        .expect("must run without privilege");
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("ok"));
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
