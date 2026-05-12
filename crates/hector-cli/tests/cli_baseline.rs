use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

// Regression: P2-6 — a corrupt `.hector/baseline.json` used to flow through
// `unwrap_or_default()` in the runner, silently producing wrong suppression
// behavior with no warning. The runner now distinguishes `NotFound` (treat
// as empty) from any other load error (warn to stderr and proceed with an
// empty baseline so the check still runs).
#[test]
fn corrupt_baseline_emits_stderr_warning() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "clean\n").unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(
        &cfg,
        "schema_version: 2\nrules:\n  noop:\n    description: x\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    )
    .unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();

    // Pre-place a malformed baseline at the expected location.
    fs::create_dir_all(dir.path().join(".hector")).unwrap();
    fs::write(dir.path().join(".hector/baseline.json"), "{not valid json").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    // The check still succeeds (corrupt baseline = treated as empty, the
    // rule passes). The user must see a warning explaining the situation.
    assert!(
        out.status.success(),
        "check should succeed with empty fallback baseline; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("baseline") && (stderr.contains("corrupt") || stderr.contains("invalid")),
        "stderr must warn about the corrupt baseline; got: {stderr}"
    );
}

// P2-6 negative case: a missing baseline file is the normal first-run
// state. The runner must NOT print a warning in that case.
#[test]
fn missing_baseline_is_silent() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "clean\n").unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(
        &cfg,
        "schema_version: 2\nrules:\n  noop:\n    description: x\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    )
    .unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();

    // No .hector/baseline.json present.
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.to_lowercase().contains("baseline"),
        "missing baseline must be silent; got: {stderr}"
    );
}

#[test]
fn baseline_skips_gitignored_and_target_dirs() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/foo.rs"),
        "fn main() { let _ = x.unwrap(); }\n",
    )
    .unwrap();
    // `target/` is in Hector's built-in skip list, so a walkdir-based impl
    // would still descend into it (reading every file) even though the skip
    // matcher later short-circuits engine.check. A gitignore-aware walker
    // must not descend at all.
    fs::create_dir_all(root.join("target/debug")).unwrap();
    fs::write(
        root.join("target/debug/junk.rs"),
        "fn main() { let _ = x.unwrap(); }\n",
    )
    .unwrap();
    // A directory that is *not* in the built-in skip globs but *is*
    // gitignored. The current walkdir impl will fingerprint it; a
    // gitignore-aware impl must skip it. This is the canonical P0-10
    // regression: real repos contain large gitignored dirs (caches, vendor
    // mirrors, generated docs) that aren't in the built-in skip list.
    fs::create_dir_all(root.join("myignored")).unwrap();
    fs::write(
        root.join("myignored/junk.rs"),
        "fn main() { let _ = x.unwrap(); }\n",
    )
    .unwrap();
    fs::write(root.join(".gitignore"), "target/\nmyignored/\n").unwrap();
    let cfg = "schema_version: 2\nrules:\n  no-unwrap:\n    description: x\n    engine: ast\n    language: rust\n    scope: [\"**/*.rs\"]\n    severity: warning\n    pattern: $E.unwrap()\n";
    let trusted = hector_core::trust::write_trust_block(cfg).unwrap();
    let cfg_path = root.join(".hector.yml");
    fs::write(&cfg_path, trusted).unwrap();
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["baseline", "--config", cfg_path.to_str().unwrap()])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let baseline: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(".hector/baseline.json")).unwrap())
            .unwrap();
    // E1: on-disk shape switched from `fingerprints: [string]` to
    // `entries: { string: option<string> }`. The keys are still the
    // tuple-fingerprint strings.
    let entries = baseline["entries"].as_object().unwrap();
    let printed: Vec<String> = entries.keys().cloned().collect();
    assert!(
        printed.iter().any(|f| f.contains("src/foo.rs")),
        "src/foo.rs must be baselined: {printed:?}"
    );
    assert!(
        !printed.iter().any(|f| f.contains("target/")),
        ".gitignored target/ must be skipped: {printed:?}"
    );
    assert!(
        !printed.iter().any(|f| f.contains("myignored/")),
        ".gitignored myignored/ must be skipped: {printed:?}"
    );
}

#[test]
fn baseline_records_and_then_filters() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "DEBUG marker\n").unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(
        &cfg,
        "schema_version: 2\nrules:\n  no-debug:\n    description: x\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"grep -nE 'DEBUG' {file} && exit 1 || exit 0\"\n",
    ).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "baseline",
            "--config",
            cfg.to_str().unwrap(),
            "--scan",
            "*.txt",
        ])
        .assert()
        .success();
    assert!(dir.path().join(".hector/baseline.json").exists());

    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(0);
}
