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

/// A MultiEdit event whose `edits` array is the raw JSON `edits` (caller owns
/// the exact shape so folding-order tests can encode multiple edits).
fn multi_edit_payload_with(file_path: &std::path::Path, edits_json: &str) -> String {
    format!(
        r#"{{"tool_name":"MultiEdit","tool_input":{{"file_path":"{}","edits":{}}}}}"#,
        file_path.display(),
        edits_json
    )
}

/// The single-edit MultiEdit event used by the allow/block reachability tests.
fn multi_edit_payload(file_path: &std::path::Path) -> String {
    multi_edit_payload_with(file_path, r#"[{"old_string":"OLD","new_string":"NEW"}]"#)
}

/// A NotebookEdit event. `new_source_json` is spliced verbatim into the JSON
/// string value (caller pre-escapes, so byte-exact bytes like a trailing `\n`
/// survive); `edit_mode` is one of replace/insert/delete.
fn notebook_edit_payload(path: &std::path::Path, new_source_json: &str, edit_mode: &str) -> String {
    format!(
        r#"{{"tool_name":"NotebookEdit","tool_input":{{"notebook_path":"{}","new_source":"{}","edit_mode":"{}"}}}}"#,
        path.display(),
        new_source_json,
        edit_mode
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

/// Task 5.24: MultiEdit is now GATED (was catch-all "not yet gated"). The hook
/// synthesizes the sequential post-edit content and pipes it to `ironlint`; a
/// clean synthesis + stub exit 0 must ALLOW (exit 0). This test was previously
/// `multi_edit_fails_closed_not_yet_gated`, which asserted the OLD catch-all
/// block — it now asserts MultiEdit REACHES ironlint like Edit/Write do.
#[test]
fn multi_edit_allow_on_exit_0() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(0, "");
    let file = fx.file("foo.py");
    std::fs::write(&file, "OLD\n").unwrap();
    fx.run("PreToolUse", &multi_edit_payload(&file), &[])
        .success()
        .code(0);
}

/// MultiEdit that synthesizes cleanly but whose gate blocks (stub exit 2) must
/// deny (exit 2) and surface the verdict message — same block path as Edit.
#[test]
fn multi_edit_block_on_exit_2_surfaces_message() {
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
    fx.run("PreToolUse", &multi_edit_payload(&file), &[])
        .failure()
        .code(2)
        .stderr(predicates::str::contains("no bare except"));
}

/// Folding correctness: the hook must apply the `edits` array **in order**,
/// each against the post-previous-edits content, and feed ironlint the exact
/// final bytes. File `A B A\n`; edit 1 replaces the unique `B`→`A` (yielding
/// `A A A\n`), edit 2 `replace_all` `A`→`Q` (yielding `Q Q Q\n`). Reversed
/// order would produce `Q A Q\n`, so asserting the captured stdin is `Q Q Q\n`
/// proves forward order + sequential application, not mere reachability.
#[test]
fn multi_edit_folds_edits_sequentially() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    let capture = fx.file("captured_stdin.txt");
    fx.stub_capturing(0, "", &capture);
    let file = fx.file("foo.py");
    std::fs::write(&file, "A B A\n").unwrap();
    let edits = r#"[{"old_string":"B","new_string":"A"},{"old_string":"A","new_string":"Q","replace_all":true}]"#;
    fx.run("PreToolUse", &multi_edit_payload_with(&file, edits), &[])
        .success()
        .code(0);
    assert_eq!(std::fs::read_to_string(&capture).unwrap(), "Q Q Q\n");
}

/// Sequential uniqueness: a non-`replace_all` edit requires exactly one match
/// **after prior edits apply**. File `A B\n`; edit 1 `B`→`A` (unique) makes
/// `A A\n`; edit 2 `A`→`C` is non-replace_all but now matches twice → the hook
/// must BLOCK (exit 2), mirroring Claude Code refusing the ambiguous edit. The
/// duplication is created by edit 1, so this also proves uniqueness is judged
/// post-fold, not against the original file.
#[test]
fn multi_edit_blocks_when_later_edit_not_unique() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    // ironlint must never be consulted — synthesis fails first.
    fx.stub(0, "");
    let file = fx.file("foo.py");
    std::fs::write(&file, "A B\n").unwrap();
    let edits = r#"[{"old_string":"B","new_string":"A"},{"old_string":"A","new_string":"C"}]"#;
    fx.run("PreToolUse", &multi_edit_payload_with(&file, edits), &[])
        .failure()
        .code(2)
        .stderr(predicates::str::contains("appears 2 times"));
}

/// An empty `edits` array has nothing to gate → allow (exit 0).
#[test]
fn multi_edit_empty_edits_allows() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    let capture = fx.file("captured_stdin.txt");
    // ironlint must never be consulted for a no-op edit set.
    fx.stub_capturing(0, "", &capture);
    let file = fx.file("foo.py");
    std::fs::write(&file, "unchanged\n").unwrap();
    fx.run("PreToolUse", &multi_edit_payload_with(&file, "[]"), &[])
        .success()
        .code(0);
    assert!(!capture.exists(), "empty edits must not reach ironlint");
}

/// Task 5.24: NotebookEdit `replace`/`insert` gates the cell's `new_source`
/// (byte-exact, including a trailing `\n`) as stdin against checks matching the
/// notebook path. Clean gate (stub 0) → allow, and the captured stdin equals
/// the exact `new_source` bytes.
#[test]
fn notebook_edit_replace_gates_new_source() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    let capture = fx.file("captured_stdin.txt");
    fx.stub_capturing(0, "", &capture);
    let nb = fx.file("nb.ipynb");
    fx.run(
        "PreToolUse",
        &notebook_edit_payload(&nb, r"cell = 1\n", "replace"),
        &[],
    )
    .success()
    .code(0);
    assert_eq!(std::fs::read_to_string(&capture).unwrap(), "cell = 1\n");
}

/// NotebookEdit whose gate blocks (stub exit 2) must deny and surface the
/// message — the cell's proposed source reached ironlint.
#[test]
fn notebook_edit_block_on_exit_2_surfaces_message() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(
        2,
        r#"{"blocks":[{"check":"g","message":"no bare except"}]}"#,
    );
    let nb = fx.file("nb.ipynb");
    fx.run(
        "PreToolUse",
        &notebook_edit_payload(&nb, r"try:\n    pass\nexcept:\n    pass\n", "replace"),
        &[],
    )
    .failure()
    .code(2)
    .stderr(predicates::str::contains("no bare except"));
}

/// NotebookEdit `edit_mode:"delete"` removes a cell — there is no proposed
/// content to gate, so the hook must ALLOW (exit 0) and never reach ironlint.
/// The capturing stub proves it: its capture file is never written.
#[test]
fn notebook_edit_delete_allows_without_gating() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    let capture = fx.file("captured_stdin.txt");
    fx.stub_capturing(0, "", &capture);
    let nb = fx.file("nb.ipynb");
    fx.run(
        "PreToolUse",
        &notebook_edit_payload(&nb, r"gone", "delete"),
        &[],
    )
    .success()
    .code(0);
    assert!(!capture.exists(), "delete must not reach ironlint");
}

/// The catch-all still fails LOUD and CLOSED for a genuinely-unknown tool (one
/// NOT in {Write, Edit, MultiEdit, NotebookEdit}). Before Task 5.24 this
/// coverage rode on MultiEdit; now that MultiEdit is gated, an actually-unknown
/// tool stands in. An ungated tool call must never be mistaken for an allowed
/// one — block (exit 2) and name the tool on stderr.
#[test]
fn unknown_tool_fails_closed_not_yet_gated() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available on this machine");
        return;
    }
    let fx = HookFixture::new(HOOK);
    // No stub exit code matters — a not-yet-gated tool must never reach ironlint.
    fx.stub(0, "");
    let file = fx.file("foo.py");
    let payload = format!(
        r#"{{"tool_name":"SomeFutureTool","tool_input":{{"file_path":"{}","content":"x"}}}}"#,
        file.display()
    );
    fx.run("PreToolUse", &payload, &[])
        .failure()
        .code(2)
        .stderr(predicates::str::contains("SomeFutureTool"))
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
