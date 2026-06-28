/// `hector check --diff` must fail loudly when a changed file named in the diff
/// cannot be read (deleted between diff-gen and check, permissions, non-UTF-8).
/// Fabricating empty content would let a real violation pass vacuously, so an
/// unreadable diff target is a hard error (exit 1), never a silent empty pass.
mod common;

use assert_cmd::Command;
use tempfile::tempdir;

// A noop gate scoped to every file — proves the failure comes from the read,
// not from a gate verdict (the gate would pass if it ever ran).
const GATE_YAML: &str = "checks:\n  noop:\n    files: [\"*\"]\n    run: \"true\"\n";

// A modification diff for a file that does not exist on disk.
const DIFF: &str = "--- a/missing.txt\n+++ b/missing.txt\n@@ -1 +1 @@\n-old\n+new\n";

#[test]
fn diff_target_that_cannot_be_read_is_a_hard_error() {
    let config_dir = tempdir().unwrap();

    let cfg = config_dir.path().join(".hector.yml");
    std::fs::write(&cfg, GATE_YAML).unwrap();

    // The diff references `missing.txt`, which is never created on disk.
    let diff_path = config_dir.path().join("changes.diff");
    std::fs::write(&diff_path, DIFF).unwrap();

    let xdg = common::blessed_store(&cfg);

    let out = Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        // Resolve the relative diff path against the config dir, where
        // `missing.txt` is guaranteed absent.
        .current_dir(config_dir.path())
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--diff",
            diff_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    // Pre-fix this read fabricated empty content → the noop gate passed → exit 0.
    // The fix surfaces the read failure as exit 1, never a vacuous pass.
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 for unreadable diff target, got {:?}",
        out.status.code()
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("missing.txt"),
        "stderr must name the unreadable file, got: {stderr}"
    );
}
