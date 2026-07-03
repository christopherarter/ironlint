/// `ironlint check --diff` must not let one unreadable/non-UTF-8 file abort
/// the whole batch. Before the fix, `run_diff`'s per-file loop returned a
/// blanket `Ok(1)` the moment `std::fs::read_to_string` failed on *any*
/// changed file — even after earlier files' verdicts (including a real
/// Block) had already been folded into the running totals. That silently
/// hid a genuine policy violation in a sibling file and misclassified the
/// tier as a config error (exit 1) instead of a Block (exit 2).
///
/// Real repos contain images, fixtures, UTF-16 files alongside the text
/// files an agent is actually editing — one binary file in a diff must not
/// suppress enforcement on everything else in the same batch.
mod common;

use assert_cmd::Command;
use tempfile::tempdir;

const NO_TODO_CFG: &str =
    "checks:\n  no-todo:\n    files: \"**/*\"\n    run: \"grep -q TODO \\\"$IRONLINT_FILE\\\" && exit 2 || exit 0\"\n";

/// A unified diff naming two changed files: a clean-looking text file that
/// (on disk) actually contains a TODO, and a binary file. Content is read
/// from disk by `run_diff`, not from the diff body, so the diff hunks below
/// are placeholders — what matters is which paths are named.
const DIFF: &str = concat!(
    "--- a/good.rs\n+++ b/good.rs\n@@ -1 +1 @@\n-old\n+new\n",
    "--- a/bad.bin\n+++ b/bad.bin\n@@ -1 +1 @@\n-old\n+new\n",
);

#[test]
fn unreadable_sibling_file_does_not_hide_a_real_block() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    std::fs::write(&cfg, NO_TODO_CFG).unwrap();

    // Good file: valid UTF-8, contains a real violation.
    let good = dir.path().join("good.rs");
    std::fs::write(&good, "// TODO fix this\n").unwrap();

    // Bad file: invalid UTF-8 bytes — read_to_string must fail on this one.
    let bad = dir.path().join("bad.bin");
    std::fs::write(&bad, [0xFF, 0xFE, 0x00, 0xFF]).unwrap();

    let diff_path = dir.path().join("changes.diff");
    std::fs::write(&diff_path, DIFF).unwrap();

    let xdg = common::blessed_store(&cfg);

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .current_dir(dir.path())
        .args([
            "check",
            "--config",
            ".ironlint.yml",
            "--diff",
            diff_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    // The good file's real Block must survive the bad file's read failure:
    // exit 2 (Block), never a blanket exit 1 that hides it.
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2 (Block from good.rs survives bad.bin's read failure), got {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bad.bin"),
        "stderr must name the skipped non-UTF-8 file, got: {stderr}"
    );
    assert!(
        stderr.contains("non_utf8"),
        "stderr must classify the skip reason as non_utf8, got: {stderr}"
    );
}

/// `--explain` surfaces the skipped file via the engine's existing
/// `ExplainOutcome::Skipped` vocabulary rather than a bespoke shape.
#[test]
fn explain_reports_the_skipped_file() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    std::fs::write(&cfg, NO_TODO_CFG).unwrap();

    let good = dir.path().join("good.rs");
    std::fs::write(&good, "// clean\n").unwrap();

    let bad = dir.path().join("bad.bin");
    std::fs::write(&bad, [0xFF, 0xFE, 0x00, 0xFF]).unwrap();

    let diff_path = dir.path().join("changes.diff");
    std::fs::write(&diff_path, DIFF).unwrap();

    let xdg = common::blessed_store(&cfg);

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .current_dir(dir.path())
        .args([
            "check",
            "--config",
            ".ironlint.yml",
            "--diff",
            diff_path.to_str().unwrap(),
            "--explain",
        ])
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(0), "good.rs is clean -> pass");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bad.bin"),
        "explain output must name the skipped file, got: {stderr}"
    );
    assert!(
        stderr.contains("skipped") && stderr.contains("non_utf8"),
        "explain output must show a Skipped{{reason: non_utf8}} row, got: {stderr}"
    );
}

/// A diff of only-good files must behave exactly as before the fix.
#[test]
fn all_readable_files_unaffected() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    std::fs::write(&cfg, NO_TODO_CFG).unwrap();

    let good = dir.path().join("good.rs");
    std::fs::write(&good, "// clean\n").unwrap();

    let diff_content = "--- a/good.rs\n+++ b/good.rs\n@@ -1 +1 @@\n-old\n+new\n";
    let diff_path = dir.path().join("changes.diff");
    std::fs::write(&diff_path, diff_content).unwrap();

    let xdg = common::blessed_store(&cfg);

    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .current_dir(dir.path())
        .args([
            "check",
            "--config",
            ".ironlint.yml",
            "--diff",
            diff_path.to_str().unwrap(),
        ])
        .assert()
        .code(0);
}
