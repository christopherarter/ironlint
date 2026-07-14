use super::*;
use crate::engine::InternalReason;
use crate::runner::{
    materialize_tmpfile, sweep_stale_tmpfiles, CheckInput, CheckOptions, ExplainOutcome,
    IronLintEngine,
};
use crate::verdict::Status;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

// --- Phase 4 test helpers ---

fn write_config(dir: &TempDir, body: &str) {
    std::fs::write(dir.path().join(".ironlint.yml"), body).unwrap();
}

fn load_with_event(dir: &TempDir, event: &str) -> IronLintEngine {
    IronLintEngine::builder()
        .with_options(CheckOptions {
            event: event.to_string(),
            ..Default::default()
        })
        .load(&dir.path().join(".ironlint.yml"))
        .unwrap()
}

fn file_input(dir: &TempDir, name: &str, content: &str) -> CheckInput {
    let path = dir.path().join(name);
    std::fs::write(&path, content).unwrap();
    CheckInput::File {
        path,
        content: content.to_string(),
    }
}

fn touch(dir: &TempDir, name: &str) {
    std::fs::write(dir.path().join(name), "").unwrap();
}

fn abs(dir: &TempDir, name: &str) -> PathBuf {
    dir.path().join(name)
}

#[test]
fn matching_check_that_exits_2_blocks() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".ironlint.yml",
        "checks:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
    );
    let target = write(dir.path(), "a.rs", "// nothing\n");
    let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
    let v = engine
        .check(CheckInput::File {
            path: target,
            content: "// TODO fix\n".into(),
        })
        .unwrap();
    assert_eq!(v.status, Status::Block);
    assert_eq!(v.blocks.len(), 1);
    assert_eq!(v.blocks[0].check, "no-todo");
}

#[test]
fn non_matching_file_passes_with_no_checks_run() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".ironlint.yml",
        "checks:\n  ts-only:\n    files: \"**/*.ts\"\n    run: \"exit 2\"\n",
    );
    let target = write(dir.path(), "a.rs", "x\n");
    let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
    let v = engine
        .check(CheckInput::File {
            path: target,
            content: "x\n".into(),
        })
        .unwrap();
    assert_eq!(v.status, Status::Pass);
    assert!(v.passed.is_empty());
}

#[test]
fn broken_check_is_internal_error() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".ironlint.yml",
        "checks:\n  oops:\n    files: \"**/*.rs\"\n    run: \"definitely-not-real-xyz\"\n",
    );
    let target = write(dir.path(), "a.rs", "x\n");
    let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
    let v = engine
        .check(CheckInput::File {
            path: target,
            content: "x\n".into(),
        })
        .unwrap();
    assert_eq!(v.status, Status::InternalError);
    assert_eq!(v.errors[0].reason, "not_found");
}

#[test]
fn block_with_no_output_uses_check_id_message() {
    // Unnamed step (plain `run:`) → "<check-id> blocked"
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".ironlint.yml",
        "checks:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n",
    );
    let target = write(dir.path(), "a.rs", "x\n");
    let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
    let v = engine
        .check(CheckInput::File {
            path: target,
            content: "x\n".into(),
        })
        .unwrap();
    assert_eq!(v.status, Status::Block);
    assert_eq!(v.blocks[0].message, "no-todo blocked");
}

#[test]
fn block_with_no_output_and_named_step_uses_step_name_in_message() {
    // Named blocking step → "<check-id> › <step-name> blocked" (spec §5)
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".ironlint.yml",
        "checks:\n  ts-quality:\n    files: \"**/*.ts\"\n    steps:\n      - name: no-any\n        run: \"exit 2\"\n",
    );
    let target = write(dir.path(), "a.ts", "x\n");
    let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
    let v = engine
        .check(CheckInput::File {
            path: target,
            content: "x\n".into(),
        })
        .unwrap();
    assert_eq!(v.status, Status::Block);
    assert_eq!(v.blocks[0].message, "ts-quality \u{203a} no-any blocked");
}

#[test]
fn explain_reports_per_check_outcome() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".ironlint.yml"),
        "checks:\n  blocker:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n  passer:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    )
    .unwrap();
    let target = dir.path().join("a.rs");
    std::fs::write(&target, "x\n").unwrap();
    let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
    let report = engine
        .check_with_explain(CheckInput::File {
            path: target,
            content: "x\n".into(),
        })
        .unwrap();
    let outcomes: std::collections::HashMap<_, _> = report
        .explain
        .iter()
        .map(|r| {
            (
                r.check_id.clone(),
                matches!(r.outcome, ExplainOutcome::Fire),
            )
        })
        .collect();
    assert!(outcomes["blocker"]);
    assert!(!outcomes["passer"]);
}

#[test]
fn check_filter_skips_unselected_checks() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".ironlint.yml"),
        "checks:\n  blocker:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n  other:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n",
    )
    .unwrap();
    let target = dir.path().join("a.rs");
    std::fs::write(&target, "x\n").unwrap();
    let mut engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
    engine.set_check_filter(std::iter::once("other".to_string()).collect());
    let v = engine
        .check(CheckInput::File {
            path: target,
            content: "x\n".into(),
        })
        .unwrap();
    assert_eq!(v.blocks.len(), 1);
    assert_eq!(v.blocks[0].check, "other");
}

#[test]
fn checks_accessor_returns_loaded_check_ids() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".ironlint.yml",
        "checks:\n  alpha:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n  beta:\n    files: \"**/*.ts\"\n    run: \"exit 0\"\n",
    );
    let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
    let ids: Vec<&str> = engine.checks().keys().map(|k| k.as_str()).collect();
    // BTreeMap iterates in key order
    assert_eq!(ids, vec!["alpha", "beta"]);
}

#[test]
fn ironlint_file_is_absolute_for_checks() {
    // ABI lock: `$IRONLINT_FILE` handed to a check is always an absolute path,
    // so a check can match it without guessing whether it's relative. The
    // check blocks (exit 2) iff `$IRONLINT_FILE` is *not* absolute; a Pass
    // verdict proves the engine resolved it to an absolute path. Guards the
    // pi-harness report that `$IRONLINT_FILE` was unexpectedly relative.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".ironlint.yml",
        "checks:\n  abs:\n    files: \"**/*.rs\"\n    run: \"case \\\"$IRONLINT_FILE\\\" in /*) exit 0;; *) exit 2;; esac\"\n",
    );
    let target = write(dir.path(), "a.rs", "x\n");
    let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
    let v = engine
        .check(CheckInput::File {
            path: target,
            content: "x\n".into(),
        })
        .unwrap();
    assert_eq!(
        v.status,
        Status::Pass,
        "$IRONLINT_FILE must be absolute (check blocks on a non-absolute path): {:?}",
        v.blocks
    );
}

#[test]
fn ironlint_files_are_absolute_for_pre_commit_set() {
    // ABI lock: `$IRONLINT_FILES` handed to a pre-commit check is always
    // newline-joined absolute paths. The check blocks (exit 2) iff any
    // entry in `$IRONLINT_FILES` is not absolute.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".ironlint.yml",
        "checks:\n  abs:\n    files: \"**/*.rs\"\n    on: [pre-commit]\n    run: \"for p in \\\"$IRONLINT_FILES\\\"; do case \\\"$p\\\" in /*) ;; *) exit 2;; esac; done\"\n",
    );
    touch(&dir, "a.rs");
    touch(&dir, "b.rs");
    let engine = load_with_event(&dir, "pre-commit");
    // Pass RELATIVE paths into check_set, simulating the CLI --diff path.
    let v = engine
        .check_set(&[PathBuf::from("a.rs"), PathBuf::from("b.rs")])
        .unwrap();
    assert_eq!(
        v.status,
        Status::Pass,
        "$IRONLINT_FILES entries must be absolute (check blocks on a non-absolute path): {:?}",
        v.blocks
    );
}

#[test]
fn disable_directive_suppresses_a_check() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".ironlint.yml"),
        "checks:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n",
    )
    .unwrap();
    let target = dir.path().join("a.rs");
    std::fs::write(&target, "x\n").unwrap();
    let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
    let v = engine
        .check(CheckInput::File {
            path: target,
            content: "// ironlint-disable: no-todo\n".into(),
        })
        .unwrap();
    assert_eq!(v.status, Status::Pass);
    assert!(v.blocks.is_empty());
}

// --- Phase 4: on: filter + pre-commit run-once ---

#[test]
fn on_filter_skips_write_only_check_at_pre_commit() {
    let dir = tempfile::tempdir().unwrap();
    write_config(
        &dir,
        "checks:\n  g:\n    files: \"*\"\n    run: \"exit 2\"\n",
    ); // on defaults to [write]
    let engine = load_with_event(&dir, "pre-commit");
    let v = engine.check(file_input(&dir, "x.txt", "b")).unwrap();
    assert_eq!(
        v.status,
        Status::Pass,
        "write-only check must not run at pre-commit"
    );
}

#[test]
fn on_filter_runs_check_subscribed_to_event() {
    let dir = tempfile::tempdir().unwrap();
    write_config(
        &dir,
        "checks:\n  g:\n    files: \"*\"\n    on: [pre-commit]\n    run: \"exit 2\"\n",
    );
    let engine = load_with_event(&dir, "pre-commit");
    let v = engine.check(file_input(&dir, "x.txt", "b")).unwrap();
    assert_eq!(
        v.status,
        Status::Block,
        "a pre-commit check must run at pre-commit"
    );
}

#[test]
fn pre_commit_runs_check_once_over_the_set() {
    let dir = tempfile::tempdir().unwrap();
    // Counter: each invocation appends one byte to runs.txt via printf.
    // The Rust assertion below is the single source of truth for run count.
    write_config(
        &dir,
        "checks:\n  g:\n    files: \"*.rs\"\n    on: [pre-commit]\n    run: \"printf x >> $IRONLINT_ROOT/runs.txt\"\n",
    );
    touch(&dir, "a.rs");
    touch(&dir, "b.rs");
    let engine = load_with_event(&dir, "pre-commit");
    let v = engine
        .check_set(&[abs(&dir, "a.rs"), abs(&dir, "b.rs")])
        .unwrap();
    let runs = std::fs::read_to_string(dir.path().join("runs.txt")).unwrap_or_default();
    assert_eq!(
        runs.len(),
        1,
        "check must run exactly once over the set, got {runs:?}"
    );
    assert_eq!(v.status, Status::Pass);
}

// --- $IRONLINT_EVENT: pin the exact value a check sees, end-to-end ---

#[test]
fn ironlint_event_seen_by_check_is_write_for_write_dispatch() {
    // Traces the real write-lifecycle path (CheckOptions.event ->
    // run_one_check -> GateEnv.event -> $IRONLINT_EVENT), not just that
    // gate.rs forwards whatever string it's given.
    let dir = tempfile::tempdir().unwrap();
    write_config(
        &dir,
        "checks:\n  g:\n    files: \"*\"\n    run: \"[ \\\"$IRONLINT_EVENT\\\" = write ] || exit 2\"\n",
    );
    let engine = load_with_event(&dir, "write");
    let v = engine.check(file_input(&dir, "x.txt", "body")).unwrap();
    assert_eq!(
        v.status,
        Status::Pass,
        "check must see IRONLINT_EVENT=write on the write dispatch path"
    );
}

#[test]
fn ironlint_event_seen_by_check_is_pre_commit_for_pre_commit_dispatch() {
    // Same, but through the pre-commit/set dispatch path (check_set),
    // which builds its own GateEnv independently of run_one_check.
    let dir = tempfile::tempdir().unwrap();
    write_config(
        &dir,
        "checks:\n  g:\n    files: \"*.rs\"\n    on: [pre-commit]\n    run: \"[ \\\"$IRONLINT_EVENT\\\" = pre-commit ] || exit 2\"\n",
    );
    touch(&dir, "a.rs");
    let engine = load_with_event(&dir, "pre-commit");
    let v = engine.check_set(&[abs(&dir, "a.rs")]).unwrap();
    assert_eq!(
        v.status,
        Status::Pass,
        "check must see IRONLINT_EVENT=pre-commit on the pre-commit dispatch path"
    );
}

// --- Phase 2: steps fail-fast ---

#[test]
fn steps_fail_fast_on_first_blocking_step() {
    // step 1 passes (exit 0), step 2 blocks (exit 2),
    // step 3 must NOT run. Use a sentinel file to prove step 3 was skipped.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".ironlint.yml",
        "checks:\n  g:\n    files: \"*\"\n    steps:\n      - run: \"true\"\n      - name: blocker\n        run: \"echo nope; exit 2\"\n      - run: \"touch ran3.txt\"\n",
    );
    let target = write(dir.path(), "x.txt", "body");
    let engine = IronLintEngine::load(&dir.path().join(".ironlint.yml")).unwrap();
    let v = engine
        .check(CheckInput::File {
            path: target,
            content: "body".into(),
        })
        .unwrap();
    assert_eq!(v.status, Status::Block);
    assert_eq!(v.blocks[0].step.as_deref(), Some("blocker"));
    assert!(
        !dir.path().join("ran3.txt").exists(),
        "step 3 ran after a block"
    );
}

mod tmpfile;
