//! Contract tests for `adapters/claude-code/hooks/hook.sh` — the PreToolUse
//! shell hook that gates `Write`/`Edit` calls through `ironlint check`.
//!
//! These spawn the real `hook.sh` under `bash`, with a *stub* `ironlint` on
//! `PATH` whose exit code is controlled per case. That stub is what's under
//! test's control; the hook's own exit-code translation (0/2/3/4 -> Claude
//! Code's allow/block semantics) is the actual thing being verified. See
//! `adapters/codex/hooks/hook.sh`'s sibling suite,
//! `hook_contract_codex.rs`, for the same contract on that harness.
//!
//! Every test runs in a temp project dir (with its own `.ironlint.yml`, so
//! the hook doesn't silently skip) and a temp `HOME`/`XDG_CONFIG_HOME`, so
//! the real trust store and the invoking user's `$HOME` are never touched
//! (see `common::HookFixture`).
#![cfg(unix)]

mod common;

use common::HookFixture;
use predicates::prelude::*;

const HOOK: &str = "adapters/claude-code/hooks/hook.sh";

fn write_payload(file_path: &std::path::Path) -> String {
    format!(
        r#"{{"tool_name":"Write","tool_input":{{"file_path":"{}","content":"print('hi')\n"}}}}"#,
        file_path.display()
    )
}

fn edit_payload(file_path: &std::path::Path) -> String {
    format!(
        r#"{{"tool_name":"Edit","tool_input":{{"file_path":"{}","old_string":"OLD","new_string":"NEW"}}}}"#,
        file_path.display()
    )
}

fn multi_edit_payload(file_path: &std::path::Path) -> String {
    format!(
        r#"{{"tool_name":"MultiEdit","tool_input":{{"file_path":"{}","edits":[{{"old_string":"OLD","new_string":"NEW"}}]}}}}"#,
        file_path.display()
    )
}

#[test]
fn write_allow_on_exit_0() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(0, "");
    let file = fx.file("foo.py");
    fx.run("PreToolUse", &write_payload(&file), &[])
        .success()
        .code(0);
}

#[test]
fn write_block_on_exit_2_surfaces_message() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(
        2,
        r#"{"blocks":[{"check":"g","message":"no bare except"}]}"#,
    );
    let file = fx.file("foo.py");
    fx.run("PreToolUse", &write_payload(&file), &[])
        .failure()
        .code(2)
        .stderr(predicates::str::contains("no bare except"));
}

#[test]
fn edit_allow_on_exit_0() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(0, "");
    let file = fx.file("foo.py");
    std::fs::write(&file, "OLD\n").unwrap();
    fx.run("PreToolUse", &edit_payload(&file), &[])
        .success()
        .code(0);
}

#[test]
fn edit_block_on_exit_2_surfaces_message() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(
        2,
        r#"{"blocks":[{"check":"g","message":"no bare except"}]}"#,
    );
    let file = fx.file("foo.py");
    std::fs::write(&file, "OLD\n").unwrap();
    fx.run("PreToolUse", &edit_payload(&file), &[])
        .failure()
        .code(2)
        .stderr(predicates::str::contains("no bare except"));
}

#[test]
fn internal_error_fails_open_by_default() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(3, "");
    let file = fx.file("foo.py");
    fx.run("PreToolUse", &write_payload(&file), &[])
        .success()
        .code(0);
}

#[test]
fn internal_error_fails_closed_when_configured() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(3, "");
    let file = fx.file("foo.py");
    fx.run(
        "PreToolUse",
        &write_payload(&file),
        &[("IRONLINT_FAIL_CLOSED_ON_INTERNAL", "1")],
    )
    .failure()
    .code(2);
}

#[test]
fn untrusted_config_blocks_with_trust_message() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(4, "");
    let file = fx.file("foo.py");
    fx.run("PreToolUse", &write_payload(&file), &[])
        .failure()
        .code(2)
        .stderr(predicates::str::contains("not trusted"));
}

/// FR-1: a matched-but-unhandled tool_name (here `MultiEdit`, which the
/// hook's registration matcher `Edit|Write` catches but the `case
/// "${TOOL_NAME}"` body never handles) must fail LOUD and CLOSED — not
/// silently pass through. Before the FR-1 fix, the `*)` catch-all was a bare
/// `exit 0`, so an ungated tool call would bypass every policy check with no
/// signal at all. The hook now blocks (exit 2) and names the offending tool
/// on stderr.
#[test]
fn multi_edit_fails_closed_not_yet_gated() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    // No stub exit code matters here — a not-yet-gated tool must never reach
    // the point of invoking `ironlint` at all.
    fx.stub(0, "");
    let file = fx.file("foo.py");
    fx.run("PreToolUse", &multi_edit_payload(&file), &[])
        .failure()
        .code(2)
        .stderr(predicates::str::contains("MultiEdit"))
        .stderr(predicates::str::contains("not yet gated"));
}

/// Malformed JSON on stdin must never crash the hook. Before the guard added
/// alongside this test, `jq`'s parse failure propagated through `set -e`
/// (pipefail) and killed the script with jq's own exit status (5) and a raw
/// `jq: parse error: ...` dump on stderr — a confusing, undocumented exit
/// code for Claude Code's PreToolUse runner to interpret. The hook now
/// validates the payload with `jq empty` up front and skips gracefully
/// (allow, exit 0) with a clear one-line note instead.
#[test]
fn malformed_json_payload_is_skipped_gracefully() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    // No stub exit code matters here — a malformed payload must never reach
    // the point of invoking `ironlint` at all.
    fx.stub(0, "");
    fx.run("PreToolUse", "{not json", &[])
        .success()
        .code(0)
        .stderr(predicates::str::contains("malformed"))
        .stderr(predicates::str::contains("parse error").not());
}

/// Task 5.23 Part 3: an Edit targeting a file whose ON-DISK bytes are not valid
/// UTF-8 must BLOCK (exit 2, fail CLOSED) with a CLEAN message. The Edit
/// branch synthesizes post-edit content by reading the current file with
/// `open(path, encoding="utf-8")`, which raised `UnicodeDecodeError` — a
/// `ValueError`, NOT an `OSError`, so the `except OSError` handler missed it and
/// Python dumped a raw traceback to stderr before falling to the `*) exit 2`
/// arm. The fix catches the decode error and prints a single clean line naming
/// UTF-8. The block DIRECTION is unchanged (undecodable stays blocked); only the
/// message text is cleaned up.
#[test]
fn edit_on_non_utf8_file_blocks_with_clean_message() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    // ironlint must never be consulted — the decode error short-circuits first.
    fx.stub(0, "");
    let file = fx.file("foo.py");
    std::fs::write(&file, b"\xff\xfe not utf8\n").unwrap();
    fx.run("PreToolUse", &edit_payload(&file), &[])
        .failure()
        .code(2)
        .stderr(predicates::str::contains("decode"))
        .stderr(predicates::str::contains("UTF-8"))
        .stderr(predicates::str::contains("Traceback").not())
        .stderr(predicates::str::contains("UnicodeDecodeError").not());
}
