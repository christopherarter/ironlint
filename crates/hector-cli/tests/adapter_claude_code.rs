//! Contract tests for the Claude Code adapter (`adapters/claude-code/hooks/`).
//!
//! These run under `cargo test` against the real compiled `hector` binary —
//! no Docker, no model. Synthetic Claude Code event JSON is piped to `hook.sh`
//! on stdin and we assert exit codes, stdout (the subagent envelope), and
//! stderr (verdict JSON).
//!
//! Claude Code is a **PostToolUse** adapter: the edit has already landed on
//! disk when the hook fires, so a `script` rule that greps the file works
//! (unlike the pre-write reasonix adapter, which needs `--content`/AST).
//!
//! `hector`, `jq`, and `bash` must be on PATH; the hook resolves its sibling
//! `synthesize_diff.sh` via `BASH_SOURCE`, so invoking the real script by
//! absolute path is sufficient.

use assert_cmd::Command as AssertCommand;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::tempdir;

const HECTOR_BIN: &str = env!("CARGO_BIN_EXE_hector");

fn adapter_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root is two levels above CARGO_MANIFEST_DIR")
        .join("adapters/claude-code")
}

fn claude_hook() -> PathBuf {
    adapter_dir().join("hooks/hook.sh")
}

fn synthesize_helper() -> PathBuf {
    adapter_dir().join("hooks/synthesize_diff.sh")
}

fn path_with_hector() -> String {
    let bindir = Path::new(HECTOR_BIN).parent().expect("binary has a parent");
    match std::env::var("PATH") {
        Ok(existing) => format!("{}:{existing}", bindir.display()),
        Err(_) => bindir.display().to_string(),
    }
}

/// Write `body` to `<dir>/.hector.yml` and fingerprint it with `hector trust`.
fn write_trusted(dir: &Path, body: &str) -> PathBuf {
    let cfg = dir.join(".hector.yml");
    std::fs::write(&cfg, body).unwrap();
    AssertCommand::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    cfg
}

/// Run the Claude Code hook in `mode` against `event`, with `project` as cwd
/// (the hook reads `PROJECT_ROOT` from `pwd`) and the test binary on PATH.
fn run_hook(project: &Path, mode: &str, event: &Value) -> (i32, String, String) {
    let mut child = Command::new("bash")
        .arg(claude_hook())
        .arg(mode)
        .current_dir(project)
        .env("PATH", path_with_hector())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn claude-code hook.sh");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `tool_input` event for a PostToolUse on `abs_path` with `new_string`.
fn edit_event(abs_path: &str, new_string: &str) -> Value {
    json!({ "tool_input": { "file_path": abs_path, "new_string": new_string } })
}

const SCRIPT_CONFIG: &str = "schema_version: 2\n\
rules:\n  \
no-debug:\n    \
description: \"no DEBUG markers in source\"\n    \
engine: script\n    \
scope: [\"*.txt\"]\n    \
severity: error\n    \
script: \"grep -nE 'DEBUG' {file} && exit 1 || exit 0\"\n";

// ===================================================================
// Core PostToolUse contract
// ===================================================================

#[test]
fn posttooluse_clean_file_allowed() {
    let dir = tempdir().unwrap();
    write_trusted(dir.path(), SCRIPT_CONFIG);
    let file = dir.path().join("clean.txt");
    std::fs::write(&file, "clean content\n").unwrap();
    let (code, _o, err) = run_hook(
        dir.path(),
        "post-tool-use",
        &edit_event(file.to_str().unwrap(), "clean content"),
    );
    assert_eq!(code, 0, "clean file must be allowed; stderr: {err}");
}

#[test]
fn posttooluse_dirty_file_blocked_exit_2() {
    let dir = tempdir().unwrap();
    write_trusted(dir.path(), SCRIPT_CONFIG);
    let file = dir.path().join("dirty.txt");
    std::fs::write(&file, "this has DEBUG in it\n").unwrap();
    let (code, _o, err) = run_hook(
        dir.path(),
        "post-tool-use",
        &edit_event(file.to_str().unwrap(), "this has DEBUG in it"),
    );
    assert_eq!(code, 2, "dirty file must block with exit 2; stderr: {err}");
}

#[test]
fn session_start_clears_stale_state() {
    let dir = tempdir().unwrap();
    write_trusted(dir.path(), SCRIPT_CONFIG);
    let hector_dir = dir.path().join(".hector");
    std::fs::create_dir_all(&hector_dir).unwrap();
    let state = hector_dir.join("session.json");
    std::fs::write(
        &state,
        r#"{"session_id":"stale","started_at":"t","edits":[]}"#,
    )
    .unwrap();
    let (code, _o, _e) = run_hook(dir.path(), "session-start", &json!({}));
    assert_eq!(code, 0);
    assert!(
        !state.exists(),
        "session-start must clear stale session.json"
    );
}

#[test]
fn stop_with_no_session_is_noop() {
    let dir = tempdir().unwrap();
    write_trusted(dir.path(), SCRIPT_CONFIG);
    let (code, _o, _e) = run_hook(dir.path(), "stop", &json!({}));
    assert_eq!(code, 0, "stop with no session.json is a no-op");
}

#[test]
fn posttooluse_untrusted_config_exit_1() {
    // An untrusted config is a config/internal error (exit 1), distinct from
    // a policy violation (exit 2).
    let dir = tempdir().unwrap();
    let cfg = write_trusted(dir.path(), SCRIPT_CONFIG);
    // Break the fingerprint so the trust gate fails at load.
    break_trust(&cfg);
    let file = dir.path().join("file.txt");
    std::fs::write(&file, "anything\n").unwrap();
    let (code, _o, _e) = run_hook(
        dir.path(),
        "post-tool-use",
        &edit_event(file.to_str().unwrap(), "anything"),
    );
    assert_eq!(
        code, 1,
        "untrusted config must return exit 1 (internal error)"
    );
}

/// Zero out the `sha256:` digest in a trusted config so trust verification
/// fails.
fn break_trust(cfg: &Path) {
    let body = std::fs::read_to_string(cfg).unwrap();
    let out = if let Some(idx) = body.find("sha256:") {
        let start = idx + "sha256:".len();
        let hexlen = body[start..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .count();
        format!(
            "{}{}{}",
            &body[..start],
            "0".repeat(64),
            &body[start + hexlen..]
        )
    } else {
        panic!("config has no sha256 fingerprint to break");
    };
    std::fs::write(cfg, out).unwrap();
}

// ===================================================================
// claude-code-subagent provider envelope branches
// ===================================================================

// `context: file` is explicit on the semantic rules. The default context is
// `diff`, but the claude-code PostToolUse hook gates with `--file` (no diff),
// so a `context: diff` rule would error with "context: diff but no diff
// provided" when the subagent deferred envelope is built. Pinning
// `context: file` exercises the envelope branches as intended.
const SUBAGENT_CONFIG: &str = "schema_version: 2\n\
llm:\n  \
provider: claude-code-subagent\n\
rules:\n  \
no-debug:\n    \
description: \"no DEBUG markers in source\"\n    \
engine: script\n    \
scope: [\"*.txt\"]\n    \
severity: error\n    \
script: \"grep -nE 'DEBUG' {file} && exit 1 || exit 0\"\n  \
prose-quality:\n    \
description: \"files should read clearly\"\n    \
engine: semantic\n    \
scope: [\"*.txt\"]\n    \
severity: warning\n    \
context: file\n  \
no-todo-comment:\n    \
description: \"no TODO comments left in committed content\"\n    \
engine: semantic\n    \
scope: [\"*.txt\"]\n    \
severity: warning\n    \
context: file\n";

/// Preamble the subagent-mode hook prepends to the deferred payload in
/// `hookSpecificOutput.additionalContext`.
const PREAMBLE: &str = "AGENTIC LINT SEMANTIC EVALUATION REQUIRED:";

#[test]
fn subagent_clean_file_emits_envelope() {
    let dir = tempdir().unwrap();
    write_trusted(dir.path(), SUBAGENT_CONFIG);
    let file = dir.path().join("clean.txt");
    std::fs::write(&file, "clean content\n").unwrap();
    let (code, out, err) = run_hook(
        dir.path(),
        "post-tool-use",
        &edit_event(file.to_str().unwrap(), "clean content"),
    );
    assert_eq!(code, 0, "subagent clean file → exit 0; stderr: {err}");
    let env: Value = serde_json::from_str(out.trim()).expect("stdout is an envelope JSON");
    assert_eq!(env["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    let ctx = env["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext is a string");
    assert!(ctx.starts_with(PREAMBLE), "missing preamble; got: {ctx}");
    let payload: Value =
        serde_json::from_str(ctx[PREAMBLE.len()..].trim()).expect("payload after preamble is JSON");
    for field in ["file", "diff", "evaluate", "_evaluator_input"] {
        assert!(
            !payload[field].is_null(),
            "payload missing `{field}`: {payload}"
        );
    }
}

#[test]
fn subagent_deterministic_block_carries_deferred_rules() {
    let dir = tempdir().unwrap();
    write_trusted(dir.path(), SUBAGENT_CONFIG);
    let file = dir.path().join("dirty.txt");
    std::fs::write(&file, "this has DEBUG in it\n").unwrap();
    let (code, out, err) = run_hook(
        dir.path(),
        "post-tool-use",
        &edit_event(file.to_str().unwrap(), "this has DEBUG"),
    );
    assert_eq!(code, 2, "deterministic block → exit 2; stderr: {err}");
    assert!(
        out.trim().is_empty(),
        "block must not emit on stdout: {out}"
    );
    let v: Value = serde_json::from_str(err.trim()).expect("stderr is verdict JSON");
    assert_eq!(v["status"], "block");
    let mut ids: Vec<&str> = v["deferred_rules"]
        .as_array()
        .expect("deferred_rules array")
        .iter()
        .map(|r| r["rule_id"].as_str().unwrap())
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        ["no-todo-comment", "prose-quality"],
        "both deferred semantic rules must surface; verdict: {v}"
    );
    assert!(
        v["deferred_rules"][0]["reason"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "deferred_rules entries need a non-empty reason"
    );
}

#[test]
fn subagent_no_semantic_no_block_is_silent() {
    // Out-of-scope file (*.md; rules are *.txt) → nothing fires.
    let dir = tempdir().unwrap();
    write_trusted(dir.path(), SUBAGENT_CONFIG);
    let file = dir.path().join("other.md");
    std::fs::write(&file, "no rules apply\n").unwrap();
    let (code, out, _e) = run_hook(
        dir.path(),
        "post-tool-use",
        &edit_event(file.to_str().unwrap(), "no rules apply"),
    );
    assert_eq!(code, 0);
    assert!(out.trim().is_empty(), "no payload → empty stdout: {out}");
}

#[test]
fn direct_api_mode_emits_no_envelope() {
    let dir = tempdir().unwrap();
    let cfg = "schema_version: 2\n\
llm:\n  \
provider: anthropic\n  \
model: claude-3-5-sonnet-20241022\n\
rules:\n  \
no-debug:\n    \
description: \"no DEBUG markers in source\"\n    \
engine: script\n    \
scope: [\"*.txt\"]\n    \
severity: error\n    \
script: \"exit 0\"\n";
    write_trusted(dir.path(), cfg);
    let file = dir.path().join("direct.txt");
    std::fs::write(&file, "clean content\n").unwrap();
    let (code, out, err) = run_hook(
        dir.path(),
        "post-tool-use",
        &edit_event(file.to_str().unwrap(), "clean content"),
    );
    assert_eq!(code, 0, "direct-API clean → exit 0; stderr: {err}");
    if let Ok(v) = serde_json::from_str::<Value>(out.trim()) {
        assert!(
            v["hookSpecificOutput"].is_null(),
            "direct-API mode must not emit an envelope: {out}"
        );
    }
}

#[test]
fn self_edit_of_policy_file_absolute_short_circuits() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(dir.path(), SUBAGENT_CONFIG);
    // Break trust: if the basename short-circuit DIDN'T run, hector would be
    // invoked against this now-untrusted config and exit 1 with an error.
    break_trust(&cfg);
    let (code, out, err) = run_hook(
        dir.path(),
        "post-tool-use",
        &edit_event(cfg.to_str().unwrap(), "anything"),
    );
    assert_eq!(code, 0, "self-edit of .hector.yml must short-circuit to 0");
    assert!(
        out.trim().is_empty() && err.trim().is_empty(),
        "must be silent"
    );
}

#[test]
fn self_edit_of_policy_file_relative_short_circuits() {
    let dir = tempdir().unwrap();
    write_trusted(dir.path(), SUBAGENT_CONFIG);
    let (code, out, err) = run_hook(
        dir.path(),
        "post-tool-use",
        &edit_event(".hector.yml", "anything"),
    );
    assert_eq!(code, 0, "relative .hector.yml self-edit → 0");
    assert!(
        out.trim().is_empty() && err.trim().is_empty(),
        "must be silent"
    );
}

#[test]
fn self_edit_of_bully_yml_short_circuits() {
    let dir = tempdir().unwrap();
    write_trusted(dir.path(), SUBAGENT_CONFIG);
    let bully = dir.path().join(".bully.yml");
    let (code, out, err) = run_hook(
        dir.path(),
        "post-tool-use",
        &edit_event(bully.to_str().unwrap(), "anything"),
    );
    assert_eq!(code, 0, ".bully.yml self-edit → 0");
    assert!(
        out.trim().is_empty() && err.trim().is_empty(),
        "must be silent"
    );
}

#[test]
fn deterministic_block_emits_exactly_one_verdict() {
    let dir = tempdir().unwrap();
    write_trusted(dir.path(), SCRIPT_CONFIG);
    let file = dir.path().join("one_block.txt");
    std::fs::write(&file, "this has DEBUG\n").unwrap();
    let (code, out, err) = run_hook(
        dir.path(),
        "post-tool-use",
        &edit_event(file.to_str().unwrap(), "this has DEBUG"),
    );
    assert_eq!(code, 2);
    assert_eq!(
        err.matches("\"status\":").count(),
        1,
        "exactly one verdict JSON on stderr; got: {err}"
    );
    assert!(
        !err.contains("AGENTIC LINT -- blocked"),
        "no bully-style block summary (cross-plugin contamination): {err}"
    );
    assert!(
        out.trim().is_empty(),
        "block must not emit on stdout: {out}"
    );
}

// ===================================================================
// synthesize_diff.sh — synthetic unified-diff helper
// ===================================================================

/// Invoke the diff-synthesis helper directly and return its stdout.
fn run_synth(file: &str, old: &str, new: &str) -> String {
    let out = Command::new("bash")
        .arg(synthesize_helper())
        .arg(file)
        .arg(old)
        .arg(new)
        .output()
        .expect("spawn synthesize_diff.sh");
    assert!(
        out.status.success(),
        "synth failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn has_line(haystack: &str, line: &str) -> bool {
    haystack.lines().any(|l| l == line)
}

#[test]
fn synth_single_line_hunk_header() {
    assert!(has_line(&run_synth("foo.ts", "", "x"), "@@ -1 +1 @@"));
}

#[test]
fn synth_multiline_old_and_new_counts() {
    assert!(has_line(
        &run_synth("foo.ts", "a\nb", "x\ny\nz"),
        "@@ -1,2 +1,3 @@"
    ));
}

#[test]
fn synth_multiline_old_single_new() {
    assert!(has_line(
        &run_synth("foo.ts", "a\nb\nc", "x"),
        "@@ -1,3 +1 @@"
    ));
}

#[test]
fn synth_empty_old_multiline_new() {
    assert!(has_line(&run_synth("foo.ts", "", "x\ny"), "@@ -1 +1,2 @@"));
}

#[test]
fn synth_scrubs_embedded_diff_headers() {
    // A new_string carrying header-looking lines must not produce real diff
    // headers that reframe the edit onto another file.
    let evil = "x\n--- a/SECRET\n+++ b/SECRET\n@@ -1 +1 @@\n+pwn";
    let out = run_synth("foo.ts", "", evil);
    assert!(
        !has_line(&out, "+++ b/SECRET"),
        "embedded +++ header survived:\n{out}"
    );
    assert!(
        !has_line(&out, "--- a/SECRET"),
        "embedded --- header survived:\n{out}"
    );
    assert!(
        has_line(&out, "--- a/foo.ts"),
        "real --- header missing:\n{out}"
    );
    assert!(
        has_line(&out, "+++ b/foo.ts"),
        "real +++ header missing:\n{out}"
    );
    let hunk_headers = out.lines().filter(|l| l.starts_with("@@ ")).count();
    assert_eq!(
        hunk_headers, 1,
        "exactly one real @@ header expected:\n{out}"
    );
}
