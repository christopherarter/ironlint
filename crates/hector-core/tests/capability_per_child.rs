//! B6 regression: `clone(2)`-per-child capability isolation. Pre-fix,
//! the first `network: false` rule unshared `CLONE_NEWNET` on the parent
//! process, blocking every subsequent rule from the network.
//!
//! These tests are Linux-only because the bug and its fix are
//! Linux-only — macOS capability enforcement is best-effort and
//! doesn't manipulate namespaces at all. On non-Linux platforms the
//! file is `cfg`-gated out entirely so the crate still compiles and
//! `cargo check` succeeds.

#![cfg(target_os = "linux")]

use hector_core::config::{Capabilities, WritesPolicy};
use hector_core::engine::capability::run_with_capabilities;
use std::path::Path;

#[test]
fn network_true_rule_keeps_network_after_network_false_rule_runs_first() {
    let cwd = Path::new(".");
    // Run a network-off rule first — pre-B6 fix, this unshared the parent
    // process and left every subsequent rule stuck inside the empty netns.
    let _ = run_with_capabilities(
        "true",
        cwd,
        &Capabilities {
            network: false,
            writes: WritesPolicy::None,
        },
    )
    .expect("network-off rule ok");

    // Now run a network-on rule that probes the loopback. If the parent's
    // netns was leaked, the loopback interface lookup will fail. We probe
    // via `/proc/net/dev` (always present, always lists `lo` when the
    // default netns is intact) so we don't depend on `ip` being installed.
    let out = run_with_capabilities(
        "cat /proc/net/dev",
        cwd,
        &Capabilities {
            network: true,
            writes: WritesPolicy::None,
        },
    )
    .expect("network-on rule ran");
    assert!(
        out.stdout.contains("lo:"),
        "network-on rule must see loopback after a prior network-off rule; \
         got stdout:\n{}\nstderr:\n{}",
        out.stdout,
        out.stderr,
    );
}

#[test]
fn parent_netns_unchanged_after_network_false_rule() {
    let pre = std::fs::read_link("/proc/self/ns/net").expect("read netns symlink");
    let _ = run_with_capabilities(
        "true",
        Path::new("."),
        &Capabilities {
            network: false,
            writes: WritesPolicy::None,
        },
    )
    .expect("network-off rule ok");
    let post = std::fs::read_link("/proc/self/ns/net").expect("read netns symlink");
    assert_eq!(
        pre, post,
        "parent netns must be unchanged after a network: false rule"
    );
}
