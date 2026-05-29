//! TDD-seed regression tests for the `script`-engine pre-write content bug.
//!
//! Bug (root-caused in `specs/2026-05-29-script-engine-prewrite-content.md`):
//! the `script` engine evaluates the **on-disk** file and ignores the
//! caller-supplied `--content`. So `hector check --file <path> --content -`
//! — the path pre-write adapters (Reasonix `PreToolUse`, OpenCode
//! `tool.execute.before`) use to gate *proposed* bytes before the edit lands
//! — runs the script tool against `<path>` on disk instead of the piped
//! proposal. `ast`/`semantic` already honor `--content` (see the AST tests in
//! `cli_check_content.rs`); `script` is the lone exception. The fix (Option A
//! in the spec, §8/§9) pipes `ctx.content` to the script subprocess's stdin.
//!
//! These two tests are written through the stable `hector check` CLI and pin
//! the CORRECT post-fix behavior, so they FAIL today. They mirror the §4
//! reproduction: a `script` rule that reads the file via `{file}` while disk
//! and proposed content are held deliberately out of sync, isolating which
//! one the engine actually inspects.
//!
//! TODO(option-a): two companion test layers belong with the implementation
//! PR, not this CLI seed:
//!   1. a `crates/hector-core/src/engine/capability.rs` unit test that a
//!      stdin-reading command receives the piped proposed content (depends on
//!      the new spawn signature that pipes to fd 0);
//!   2. Reasonix-adapter golden-content tests asserting byte-exact piped
//!      content with newline fidelity through `hook.sh`'s write_file/edit_file
//!      paths.

use assert_cmd::Command;
use tempfile::tempdir;

/// Writes `.hector.yml` and trusts it with the same binary under test, in the
/// temp dir — matching the `write_trusted` idiom in `cli_check_content.rs`.
fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let cfg = dir.join(".hector.yml");
    std::fs::write(&cfg, body).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    cfg
}

/// A single `script` rule (schema v2) that blocks any file containing the
/// token `FORBIDDEN`. The command reads from stdin (no `{file}` argument), so
/// it inspects whatever bytes the engine pipes to the subprocess — the proposed
/// `--content` in Option A. `{file}` is intentionally absent: with no file
/// argument, grep reads stdin, which is where the script engine pipes the
/// proposed content. This is the correct stdin-form rule shape for pre-write
/// gating.
fn no_forbidden_config() -> &'static str {
    "schema_version: 2\n\
     rules:\n  \
       no-forbidden:\n    \
         description: \"File must not contain the token FORBIDDEN.\"\n    \
         engine: script\n    \
         scope: [\"**/*.txt\"]\n    \
         severity: error\n    \
         output: passthrough\n    \
         script: \"! grep -q FORBIDDEN\"\n"
}

/// Proposed content is checked, not disk (block case).
///
/// Disk holds CLEAN text; the proposed `--content` (stdin) contains
/// `FORBIDDEN`. The proposal violates the rule, so the check must Block
/// (exit 2 / status "block").
///
/// FAILS today: the `script` engine reads the clean disk file via `{file}`,
/// so `grep` finds nothing, the script exits 0, and the check Passes
/// (exit 0 / status "pass") — letting forbidden proposed content through.
#[test]
fn script_proposed_content_is_checked_not_disk_blocks() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let file = root.join("foo.txt");
    // On disk: CLEAN (a plain --file run would Pass).
    std::fs::write(&file, "clean content\n").unwrap();
    let cfg = write_trusted(root, no_forbidden_config());

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--content",
            "-",
            "--format",
            "json",
        ])
        // Proposed content (stdin) CONTAINS FORBIDDEN -> must block.
        .write_stdin("FORBIDDEN\n")
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(parsed["status"], "block");
}

/// Clean proposed content passes despite dirty disk (pass case).
///
/// Disk holds `FORBIDDEN`; the proposed `--content` (stdin) is clean. The
/// proposal satisfies the rule, so the check must Pass (exit 0 / status
/// "pass"). Proves the disk read is fully bypassed, not merely shadowed.
///
/// FAILS today: the `script` engine reads the dirty disk file via `{file}`,
/// so `grep` matches `FORBIDDEN`, the script exits non-zero, and the check
/// Blocks (exit 2 / status "block") — rejecting a clean proposed edit.
#[test]
fn script_clean_proposed_content_passes_despite_dirty_disk() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let file = root.join("foo.txt");
    // On disk: dirty (a plain --file run would Block).
    std::fs::write(&file, "FORBIDDEN\n").unwrap();
    let cfg = write_trusted(root, no_forbidden_config());

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--content",
            "-",
            "--format",
            "json",
        ])
        // Proposed content (stdin) is CLEAN -> must pass.
        .write_stdin("clean\n")
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(parsed["status"], "pass");
}
