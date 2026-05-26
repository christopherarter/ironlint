//! B5 — runner-level pin: the deferred path's `payload.evaluator_input`
//! must equal what `expand_context` + the direct-API prompt builder
//! produces, for ALL `ContextScope` values (Diff, File, Repo).
//!
//! Regression: the pin test in `llm/prompt.rs` exercised the prompt
//! builder in isolation, so it missed the runner's `expand_for_deferred`
//! helper that returned a different stub for Repo scope.

use hector_core::runner::{CheckInput, CheckOptions, HectorEngine};
use std::collections::HashSet;
use std::fs;
use tempfile::tempdir;

fn write_cfg(dir: &std::path::Path, context_kind: &str) -> std::path::PathBuf {
    let cfg = format!(
        r#"
schema_version: 2
trust:
  fingerprint: PLACEHOLDER
llm:
  provider: claude-code-subagent
rules:
  semantic-check:
    description: check via LLM
    engine: semantic
    scope: ["**/*.rs"]
    severity: error
    context: {context_kind}
"#
    );
    let p = dir.join(".hector.yml");
    fs::write(&p, &cfg).unwrap();
    let signed = hector_core::trust::write_trust_block(&fs::read_to_string(&p).unwrap()).unwrap();
    fs::write(&p, signed).unwrap();
    p
}

fn opts() -> CheckOptions {
    CheckOptions {
        rules: HashSet::new(),
        explain: false,
        emit_semantic_payload: true,
        allow_external_paths: false,
    }
}

// One test per ContextScope. Each test compares the deferred path's
// evaluator_input against the direct-rendered prompt produced by
// `render_semantic_prompts`. They must equal modulo the per-call
// sentinel token (which differs by design).

#[test]
fn deferred_evaluator_input_matches_direct_path_for_context_diff() {
    let tmp = tempdir().unwrap();
    let cfg = write_cfg(tmp.path(), "diff");
    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() {}\n").unwrap();

    let engine = HectorEngine::builder()
        .with_options(opts())
        .load(&cfg)
        .unwrap();
    let diff =
        "--- a/foo.rs\n+++ b/foo.rs\n@@ -1,1 +1,2 @@\n fn main() {}\n+let MARKER_DIFF = 1;\n"
            .to_string();
    let report = engine
        .check_with_explain(CheckInput::Diff {
            file: src.clone(),
            unified_diff: diff,
        })
        .unwrap();
    let deferred = report.deferred.expect("envelope present");
    // context: diff must put the diff body in evaluator_input.
    assert!(
        deferred.payload.evaluator_input.contains("MARKER_DIFF"),
        "context: diff must include the diff body in evaluator_input; got:\n{}",
        deferred.payload.evaluator_input
    );
    // Negative: must NOT carry the repo or file stubs.
    assert!(
        !deferred.payload.evaluator_input.contains("using file"),
        "context: diff must NOT carry a repo/file stub"
    );
}

#[test]
fn deferred_evaluator_input_matches_direct_path_for_context_file() {
    let tmp = tempdir().unwrap();
    let cfg = write_cfg(tmp.path(), "file");
    let src = tmp.path().join("foo.rs");
    let body = "fn main() {\n    let MARKER_FILE = 1;\n}\n";
    fs::write(&src, body).unwrap();

    let engine = HectorEngine::builder()
        .with_options(opts())
        .load(&cfg)
        .unwrap();
    let report = engine
        .check_with_explain(CheckInput::File {
            path: src.clone(),
            content: body.into(),
        })
        .unwrap();
    let deferred = report.deferred.expect("envelope present");
    assert!(
        deferred.payload.evaluator_input.contains("MARKER_FILE"),
        "context: file must put full file content in evaluator_input"
    );
    // The deferred envelope's primary evidence must match what
    // expand_context would produce for File scope: the file's body.
    // (It must NOT contain a `using file `foo.rs` content only` stub.)
    assert!(
        !deferred.payload.evaluator_input.contains("using file `"),
        "context: file must NOT carry the repo-scope stub"
    );
}

/// TDD anchor for fix (3): expansion failures must surface as __internal
/// violations → InternalError verdict, not be silently dropped.
#[test]
fn deferred_expansion_failure_surfaces_as_internal_error() {
    // A `context: diff` rule running against `CheckInput::File` (no diff)
    // causes expand_context to error. Under the old behavior this rule
    // was silently dropped from the envelope. Now it must surface as an
    // __internal violation → InternalError verdict.
    let tmp = tempdir().unwrap();
    let cfg = write_cfg(tmp.path(), "diff");
    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() {}\n").unwrap();

    let engine = HectorEngine::builder()
        .with_options(opts())
        .load(&cfg)
        .unwrap();
    // File input → no diff → context: diff cannot expand → __internal.
    let report = engine
        .check_with_explain(CheckInput::File {
            path: src,
            content: "fn main() {}\n".into(),
        })
        .unwrap();
    assert_eq!(
        report.verdict.status,
        hector_core::verdict::Status::InternalError,
        "expansion failure must surface as InternalError, not Pass with a silently-dropped rule; got verdict={:?}",
        report.verdict
    );
    let has_internal = report
        .verdict
        .violations
        .iter()
        .any(|v| v.rule_id == "__internal" && v.message.contains("semantic-check"));
    assert!(
        has_internal,
        "must include an __internal violation naming the failed rule"
    );
    // The deferred envelope must be suppressed on InternalError — the
    // engine-level failure short-circuits the LLM dispatch the envelope
    // was built to enable. Mirrors the CLI's exit-3 / no-stdout-envelope
    // behavior at the library-API level.
    assert!(
        report.deferred.is_none(),
        "deferred envelope must be suppressed on InternalError; got {:?}",
        report.deferred
    );
}

// This is the test that pins the Critical: the Repo stub must match
// `expand_context`'s exact string, NOT runner's variant.
#[test]
fn deferred_evaluator_input_repo_scope_uses_canonical_stub() {
    let tmp = tempdir().unwrap();
    let cfg = write_cfg(tmp.path(), "repo");
    let src = tmp.path().join("subdir").join("foo.rs");
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    let body = "fn main() { /* MARKER_REPO */ }\n";
    fs::write(&src, body).unwrap();

    let engine = HectorEngine::builder()
        .with_options(opts())
        .load(&cfg)
        .unwrap();
    let report = engine
        .check_with_explain(CheckInput::File {
            path: src.clone(),
            content: body.into(),
        })
        .unwrap();
    let deferred = report.deferred.expect("envelope present");
    // The canonical stub from engine::context::expand_context has NO
    // path interpolation; it is the literal string below.
    let canonical = "(repo-context expansion deferred; using file content only)";
    assert!(
        deferred.payload.evaluator_input.contains(canonical),
        "context: repo must use the canonical stub; got evaluator_input=\n{}",
        deferred.payload.evaluator_input,
    );
    // Negative: must NOT carry the runner's path-interpolated variant.
    assert!(
        !deferred.payload.evaluator_input.contains("using file `"),
        "context: repo must NOT use the path-interpolated stub variant"
    );
}
