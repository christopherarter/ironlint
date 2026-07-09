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

const HOOK: &str = "adapters/codex/hooks/hook.sh";

/// A one-file Add-File apply_patch payload touching `foo.py` (in `.ironlint.yml`
/// scope). `cwd` is the temp project so the hook resolves the config + root.
fn add_payload(cwd: &std::path::Path) -> String {
    add_payload_for(cwd, "foo.py")
}

/// A one-file Add-File apply_patch payload touching `path` (relative to cwd).
fn add_payload_for(cwd: &std::path::Path, path: &str) -> String {
    let patch = format!(
        "*** Begin Patch\n*** Add File: {}\n+print('hi')\n*** End Patch\n",
        path
    );
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

/// A Bash PreToolUse event. codex emits `tool_name:"Bash"` (capital B —
/// confirmed empirically 2026-07-06) for shell commands, with the command in
/// `tool_input.command` (same jq path as claude-code). `cwd` is required so the
/// hook resolves the project root + config.
fn bash_payload(cwd: &std::path::Path, command: &str) -> String {
    serde_json::json!({
        "tool_name": "Bash",
        "cwd": cwd.display().to_string(),
        "tool_input": { "command": command },
    })
    .to_string()
}

/// A two-file Add-File apply_patch payload touching both `foo.py` and
/// `bar.py` (both in `.ironlint.yml` scope) in a single envelope.
fn multi_add_payload(cwd: &std::path::Path) -> String {
    let patch = "*** Begin Patch\n*** Add File: foo.py\n+print('hi')\n*** Add File: bar.py\n+print('bye')\n*** End Patch\n";
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
    assert!(
        reason.contains(needle),
        "reason {reason:?} lacks {needle:?}"
    );
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
    fx.stub(
        2,
        r#"{"blocks":[{"check":"g","message":"no print statements"}]}"#,
    );
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
    fx.stub(
        2,
        r#"{"blocks":[{"check":"g","message":"blocked update"}]}"#,
    );
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
        "tool_name": "SomeOtherTool",
        "cwd": fx.project.path().display().to_string(),
        "tool_input": { "command": "echo hi" },
    })
    .to_string();
    fx.run("pre-tool-use", &payload, &[])
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}

/// After the gates→scripts rename, apply_patch additions under
/// .ironlint/scripts/ short-circuit the gate exactly like .ironlint.yml edits —
/// a mid-edit policy script's on-disk bytes won't match the trusted hash, so
/// checking it would surface a misleading "internal error". The short-circuit
/// must be PATH-ANCHORED so src/.ironlint/scripts/foo.sh (not the policy
/// surface) is NOT matched.
#[test]
fn add_to_scripts_dir_short_circuits_without_check() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    let capture = fx.file("captured_stdin.txt");
    fx.stub_capturing(0, "", &capture);
    // Canonicalize so the event cwd matches the hook's PROJECT_ROOT (on macOS
    // $(pwd) resolves /var/folders -> /private/var/folders).
    let project = std::fs::canonicalize(fx.project.path()).unwrap();
    fx.run(
        "pre-tool-use",
        &add_payload_for(&project, ".ironlint/scripts/lint.sh"),
        &[],
    )
    .success()
    .code(0)
    .stdout(predicates::str::is_empty());
    assert!(
        !capture.exists(),
        "hook must short-circuit — ironlint check was invoked (capture file exists)"
    );
}

/// Path-anchor sanity check: a file at src/.ironlint/scripts/foo.sh is NOT
/// the project's policy surface, so it must be gated normally (the stub
/// capturing path proves ironlint WAS invoked).
#[test]
fn add_to_nested_scripts_dir_is_gated() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    let capture = fx.file("captured_stdin.txt");
    fx.stub_capturing(0, "", &capture);
    let project = std::fs::canonicalize(fx.project.path()).unwrap();
    fx.run(
        "pre-tool-use",
        &add_payload_for(&project, "src/.ironlint/scripts/lint.sh"),
        &[],
    )
    .success()
    .code(0)
    .stdout(predicates::str::is_empty());
    assert!(
        capture.exists(),
        "nested src/.ironlint/scripts/foo.sh must NOT short-circuit — it is not the policy surface"
    );
}

// --- Bash branch (bash-gate self-trust prevention) ---------------------------
//
// codex emits tool_name:"Bash" for shell commands. The Bash branch runs BEFORE
// the apply_patch-only gate (which would otherwise allow every non-apply_patch
// tool). Substring pre-filter (ironlint | .ironlint) skips the spawn for
// ordinary commands; on a hit, pipes `tool_input.command` to `ironlint gate-bash`
// and translates exit 0 → allow, exit 2 → deny (deny-JSON/exit-0 per codex's
// contract), anything else → fail-closed deny. Reuses the existing deny().

/// `ls` never mentions ironlint → the pre-filter skips the spawn entirely. The
/// stub is a TRAP (exit 2): if the hook wrongly spawned, this would deny. Allow
/// (exit 0, empty stdout) proves the pre-filter short-circuit.
#[test]
fn bash_allows_benign_command() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(2, "trap");
    fx.run("pre-tool-use", &bash_payload(fx.project.path(), "ls"), &[])
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}

/// `ironlint trust` hits the pre-filter and the stubbed gate-bash exits 2 with
/// the reason → the hook must emit a deny verdict (exit 0, deny JSON) whose
/// reason carries the gate-bash message.
#[test]
fn bash_blocks_ironlint_trust() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(2, "ironlint trust must be run by a human");
    let out = fx
        .run(
            "pre-tool-use",
            &bash_payload(fx.project.path(), "ironlint trust"),
            &[],
        )
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "ironlint trust must be run by a human");
}

/// A Bash redirect onto `.ironlint.yml` hits the pre-filter (`.ironlint`) and
/// the stubbed gate-bash exits 2 → deny.
#[test]
fn bash_blocks_redirect_to_ironlint_yml() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(2, "policy files must be edited through the Write/Edit tool");
    let out = fx
        .run(
            "pre-tool-use",
            &bash_payload(fx.project.path(), "echo x > .ironlint.yml"),
            &[],
        )
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(
        &out,
        "policy files must be edited through the Write/Edit tool",
    );
}

/// If `ironlint` is not on PATH (no stub), the spawn fails. codex must emit a
/// deny (fail-closed), not allow — a broken deny check is never a silent allow.
#[test]
fn bash_fails_closed_when_ironlint_missing() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    // No stub → ironlint not on PATH → spawn fails.
    let out = fx
        .run(
            "pre-tool-use",
            &bash_payload(fx.project.path(), "ironlint trust"),
            &[],
        )
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "fail-closed");
}

/// End-to-end against the REAL `ironlint gate-bash` (not the stub): proves the
/// hook pipes `tool_input.command` to the real subcommand and the real matcher
/// blocks `ironlint trust`. The stub tests prove the hook's deny-JSON
/// translation; this one proves the integration wiring.
#[test]
fn bash_blocks_ironlint_trust_with_real_binary() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let ironlint = assert_cmd::cargo::cargo_bin("ironlint");
    let fx = common::RealBinFixture::new(HOOK, &ironlint);
    let out = fx
        .run(
            "pre-tool-use",
            &bash_payload(fx.project.path(), "ironlint trust"),
            &[],
        )
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "ironlint trust must be run by a human");
}

/// A patch touching two files in one envelope must run the per-file gate
/// loop against both manifest entries — single-file tests can't exercise
/// that the loop correctly walks a multi-line manifest and still blocks.
#[test]
fn multi_file_patch_blocks_when_any_file_blocks() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(
        2,
        r#"{"blocks":[{"check":"g","message":"blocked one of the files"}]}"#,
    );
    let out = fx
        .run("pre-tool-use", &multi_add_payload(fx.project.path()), &[])
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "blocked one of the files");
}

/// A well-formed `*** Begin Patch … *** End Patch` envelope whose only
/// section uses an unrecognized op (not Add/Update/Delete File) must fail
/// CLOSED — an empty manifest from an unrecognized op must never be
/// mistaken for the legitimate "delete-only, nothing to gate" allow path.
/// `ironlint` is stubbed to allow (exit 0) and must never be consulted.
#[test]
fn envelope_without_recognized_op_fails_closed() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(0, "");
    let payload = serde_json::json!({
        "tool_name": "apply_patch",
        "cwd": fx.project.path().display().to_string(),
        "tool_input": { "command": "*** Begin Patch\n*** Frobnicate File: foo.py\n*** End Patch" },
    })
    .to_string();
    let out = fx
        .run("pre-tool-use", &payload, &[])
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "recognized");
}

/// Feeds a byte-verbatim, real-Codex-captured payload (`tests/fixtures/codex/
/// apply_patch_add.json`) through the hook, so those fixtures stop being
/// orphaned ground-truth files nothing exercises.
#[test]
fn real_captured_add_fixture_blocks_on_exit_2() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(
        2,
        r#"{"blocks":[{"check":"g","message":"blocked captured add"}]}"#,
    );
    let raw = std::fs::read_to_string(common::repo_path(
        "tests/fixtures/codex/apply_patch_add.json",
    ))
    .expect("fixture must exist");
    let payload = raw.replace("__CWD__", &fx.project.path().display().to_string());
    let out = fx
        .run("pre-tool-use", &payload, &[])
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "blocked captured add");
}

/// Task 5.23 Part 3: an Update-File patch targeting a file whose ON-DISK bytes
/// are not valid UTF-8 must DENY (fail CLOSED) with a CLEAN reason. The
/// synthesizer's `apply_update` reads the file with `open(..., encoding="utf-8")`
/// and raised `UnicodeDecodeError` — a `ValueError`, NOT an `OSError`, so the
/// `except OSError` handler missed it and Python died with a raw traceback that
/// then rode into the deny reason. The fix catches the decode error and emits a
/// single clean line naming UTF-8. The deny DIRECTION is unchanged (undecodable
/// stays blocked); only the reason text is cleaned up.
#[test]
fn update_on_non_utf8_file_denies_with_clean_reason() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    // ironlint must never be consulted — the decode error short-circuits first.
    fx.stub(0, "");
    std::fs::write(fx.file("foo.py"), b"\xff\xfe not utf8\n").unwrap();
    let out = fx
        .run("pre-tool-use", &update_payload(fx.project.path()), &[])
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("deny must be well-formed JSON");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = v["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap_or_default();
    assert!(
        reason.contains("UTF-8"),
        "reason must name UTF-8: {reason:?}"
    );
    assert!(
        reason.contains("decode"),
        "reason must mention decode: {reason:?}"
    );
    assert!(
        !reason.contains("Traceback"),
        "reason must not leak a Python traceback: {reason:?}"
    );
    assert!(
        !reason.contains("UnicodeDecodeError"),
        "reason must not leak the raw exception name: {reason:?}"
    );
}
