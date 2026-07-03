/// `ironlint check --diff` must not fabricate empty content for a changed
/// file named in the diff that cannot actually be read (deleted between
/// diff-gen and check, permissions, non-UTF-8) -- that would let a real
/// violation pass vacuously. But it also must not abort the whole batch over
/// one bad file (see `cli_diff_skips_unreadable_file.rs` for the sibling-file
/// case): an unreadable diff target is a per-file SKIP, loudly noted on
/// stderr, and the aggregate verdict is still computed from whatever files
/// *could* be read.
mod common;

use assert_cmd::Command;
use tempfile::tempdir;

// A noop gate scoped to every file -- proves a pass here comes from the file
// being skipped (never handed to the gate), not from a gate verdict.
const GATE_YAML: &str = "checks:\n  noop:\n    files: [\"*\"]\n    run: \"true\"\n";

// A modification diff for a file that does not exist on disk.
const DIFF: &str = "--- a/missing.txt\n+++ b/missing.txt\n@@ -1 +1 @@\n-old\n+new\n";

#[test]
fn diff_target_that_cannot_be_read_is_skipped_not_a_vacuous_pass_or_hard_error() {
    let config_dir = tempdir().unwrap();

    let cfg = config_dir.path().join(".ironlint.yml");
    std::fs::write(&cfg, GATE_YAML).unwrap();

    // The diff references `missing.txt`, which is never created on disk.
    let diff_path = config_dir.path().join("changes.diff");
    std::fs::write(&diff_path, DIFF).unwrap();

    let xdg = common::blessed_store(&cfg);

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .current_dir(config_dir.path())
        // Resolve the relative diff path against the config dir, where
        // `missing.txt` is guaranteed absent.
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--diff",
            diff_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    // `missing.txt` is the only changed file and it's skipped (not fed to
    // the gate as "", and not a blanket batch-abort either): no blocks, no
    // errors -> Pass -> exit 0. The skip must still be loudly noted below.
    assert_eq!(
        out.status.code(),
        Some(0),
        "expected exit 0 (the only changed file is skipped, not vacuously passed \
         through the gate or hard-errored), got {:?}",
        out.status.code()
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("missing.txt"),
        "stderr must name the unreadable file, got: {stderr}"
    );
    assert!(
        stderr.contains("unreadable"),
        "stderr must classify the skip reason, got: {stderr}"
    );
}
