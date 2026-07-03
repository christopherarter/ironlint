//! Contract tests for `adapters/codex/hooks/hook.sh` — the PreToolUse hook
//! that gates Codex `apply_patch` edits through `ironlint check`.
//!
//! Unlike the claude-code hook (exit-code block), Codex blocks ONLY via a
//! `permissionDecision:"deny"` JSON on stdout (exit 0); malformed stdout fails
//! OPEN. These tests therefore assert the JSON shape, not the exit code, and
//! parse stdout with serde_json so a future edit that emits garbage-on-block
//! (→ fail-open) is caught. Same harness as `hook_contract_claude_code.rs`:
//! real `hook.sh` under `bash`, a stub `ironlint` on PATH, temp HOME/project.
#![cfg(unix)]

mod common;

use common::HookFixture;
use predicates::prelude::*;

const HOOK: &str = "adapters/codex/hooks/hook.sh";

/// A one-file Add-File apply_patch payload touching `foo.py` (in `.ironlint.yml`
/// scope). `cwd` is the temp project so the hook resolves the config + root.
fn add_payload(cwd: &std::path::Path) -> String {
    let patch = "*** Begin Patch\n*** Add File: foo.py\n+print('hi')\n*** End Patch\n";
    serde_json::json!({
        "tool_name": "apply_patch",
        "cwd": cwd.display().to_string(),
        "tool_input": { "command": patch },
    })
    .to_string()
}

/// An Update-File payload changing `print('old')` → `print('new')` in foo.py.
fn update_payload(cwd: &std::path::Path) -> String {
    let patch = "*** Begin Patch\n*** Update File: foo.py\n@@\n-print('old')\n+print('new')\n*** End Patch\n";
    serde_json::json!({
        "tool_name": "apply_patch",
        "cwd": cwd.display().to_string(),
        "tool_input": { "command": patch },
    })
    .to_string()
}

/// Assert stdout is a well-formed Codex deny verdict whose reason contains `needle`.
fn assert_deny(stdout: &[u8], needle: &str) {
    let v: serde_json::Value =
        serde_json::from_slice(stdout).expect("hook stdout must be well-formed JSON on block");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = v["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap_or_default();
    assert!(reason.contains(needle), "reason {reason:?} lacks {needle:?}");
}

#[test]
fn add_allow_on_exit_0_emits_empty_stdout() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(0, "");
    fx.run("pre-tool-use", &add_payload(fx.project.path()), &[])
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}

#[test]
fn add_block_on_exit_2_emits_deny_json() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(2, r#"{"blocks":[{"check":"g","message":"no print statements"}]}"#);
    let out = fx
        .run("pre-tool-use", &add_payload(fx.project.path()), &[])
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "no print statements");
}

#[test]
fn update_block_on_exit_2_emits_deny_json() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    std::fs::write(fx.file("foo.py"), "print('old')\n").unwrap();
    fx.stub(2, r#"{"blocks":[{"check":"g","message":"blocked update"}]}"#);
    let out = fx
        .run("pre-tool-use", &update_payload(fx.project.path()), &[])
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "blocked update");
}

#[test]
fn untrusted_config_emits_trust_deny() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(4, "");
    let out = fx
        .run("pre-tool-use", &add_payload(fx.project.path()), &[])
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "not trusted");
}

#[test]
fn internal_error_allows_by_default() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(3, "");
    fx.run("pre-tool-use", &add_payload(fx.project.path()), &[])
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}

#[test]
fn internal_error_denies_when_fail_closed() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(3, "");
    let out = fx
        .run(
            "pre-tool-use",
            &add_payload(fx.project.path()),
            &[("IRONLINT_FAIL_CLOSED_ON_INTERNAL", "1")],
        )
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "fail-closed");
}

#[test]
fn unparseable_patch_fails_closed() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(0, ""); // ironlint must never even be consulted
    let bad = serde_json::json!({
        "tool_name": "apply_patch",
        "cwd": fx.project.path().display().to_string(),
        "tool_input": { "command": "this is not a patch envelope" },
    })
    .to_string();
    let out = fx
        .run("pre-tool-use", &bad, &[])
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "apply_patch");
}

#[test]
fn delete_only_patch_allows() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(2, ""); // even if ironlint would block, a delete has nothing to gate
    let patch = "*** Begin Patch\n*** Delete File: foo.py\n*** End Patch\n";
    let payload = serde_json::json!({
        "tool_name": "apply_patch",
        "cwd": fx.project.path().display().to_string(),
        "tool_input": { "command": patch },
    })
    .to_string();
    fx.run("pre-tool-use", &payload, &[])
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}

#[test]
fn malformed_stdin_is_allowed_gracefully() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(0, "");
    fx.run("pre-tool-use", "{not json", &[])
        .success()
        .code(0)
        .stdout(predicates::str::is_empty())
        .stderr(predicates::str::contains("malformed"));
}

#[test]
fn non_apply_patch_tool_is_allowed() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(2, "");
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "cwd": fx.project.path().display().to_string(),
        "tool_input": { "command": "echo hi" },
    })
    .to_string();
    fx.run("pre-tool-use", &payload, &[])
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}
