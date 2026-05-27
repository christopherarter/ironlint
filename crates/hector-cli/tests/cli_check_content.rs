//! `hector check --file ... --content` end-to-end coverage.
//!
//! Closes the PreToolUse gap documented in
//! `specs/2026-05-25-reasonix-adapter.md` §4: adapters for tools without a
//! blocking PostToolUse hook (Reasonix, OpenCode `tool.execute.before`) need
//! to evaluate **proposed** post-edit content before the agent writes it to
//! disk. `--content` feeds that content while `--file` keeps the real path
//! so scope rules, baseline matching, and AST language detection all work
//! against the project's actual layout.

use assert_cmd::Command;
use tempfile::tempdir;

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

/// AST config used across most tests: blocks `panic!(...)` in Rust files
/// under `src/`.
fn no_panic_config() -> &'static str {
    "schema_version: 2\n\
     rules:\n  \
       no-panic:\n    \
         description: no panics\n    \
         engine: ast\n    \
         language: rust\n    \
         scope: [\"src/**/*.rs\"]\n    \
         severity: error\n    \
         pattern: panic!($$$)\n"
}

/// `--content <inline>` must be evaluated by the AST engine instead of the
/// on-disk file. The disk holds clean source (a plain `--file` check would
/// Pass); the proposed content contains `panic!()` and must Block.
#[test]
fn content_inline_overrides_disk_file() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let file = root.join("src/foo.rs");
    std::fs::write(&file, "fn a() {}\n").unwrap();
    let cfg = write_trusted(root, no_panic_config());

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--content",
            "fn b() { panic!(); }\n",
            "--format",
            "json",
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(parsed["status"], "block");
    assert_eq!(parsed["violations"][0]["rule_id"], "no-panic");
}

/// Inverse of the above: clean proposed content over a *bad* on-disk file
/// must Pass. Proves the disk read is fully bypassed, not just shadowed.
#[test]
fn content_inline_replaces_dirty_disk_with_clean_proposal() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let file = root.join("src/foo.rs");
    // On disk: dirty (would Block on a plain --file run).
    std::fs::write(&file, "fn a() { panic!(); }\n").unwrap();
    let cfg = write_trusted(root, no_panic_config());

    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--content",
            "fn b() {}\n",
            "--format",
            "json",
        ])
        .assert()
        .code(0);
}

/// `--content -` reads the proposed bytes from stdin. This is the documented
/// path in the adapter sketch (spec §6: `printf '%s' "$PROPOSED" | hector
/// check --file ... --content -`). argv has OS-level size limits; stdin
/// does not.
#[test]
fn content_dash_reads_from_stdin() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let file = root.join("src/foo.rs");
    std::fs::write(&file, "fn a() {}\n").unwrap();
    let cfg = write_trusted(root, no_panic_config());

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
        .write_stdin("fn b() { panic!(); }\n")
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(parsed["status"], "block");
    assert_eq!(parsed["violations"][0]["rule_id"], "no-panic");
}

/// Scope matching keys off the `--file` path, not the content. A file
/// outside `src/` must Pass even with dirty proposed content — otherwise
/// `scope: src/**/*.rs` would be meaningless for the adapter use case.
#[test]
fn content_honors_scope_against_file_path() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("scratch")).unwrap();
    // File is outside `src/` — out of scope for the rule.
    let file = root.join("scratch/foo.rs");
    std::fs::write(&file, "fn a() {}\n").unwrap();
    let cfg = write_trusted(root, no_panic_config());

    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--content",
            "fn b() { panic!(); }\n",
            "--format",
            "json",
        ])
        .assert()
        .code(0);
}

/// `// hector-disable: no-panic` in the proposed content must be honored by
/// the runner's disable map. The directive is parsed from the content
/// hector evaluates, so a PreToolUse adapter can let agents suppress with
/// the same syntax they'd use post-edit.
#[test]
fn content_disable_directive_in_proposal_is_applied() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let file = root.join("src/foo.rs");
    std::fs::write(&file, "fn a() {}\n").unwrap();
    let cfg = write_trusted(root, no_panic_config());

    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--content",
            "fn b() { panic!(); } // hector-disable: no-panic\n",
            "--format",
            "json",
        ])
        .assert()
        .code(0);
}

/// `--content` without `--file` is meaningless: scope matching and
/// language detection both require a path. Clap-level rejection (exit 2
/// for arg errors) — no engine runs.
#[test]
fn content_without_file_is_rejected() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let cfg = write_trusted(root, no_panic_config());

    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--content",
            "fn b() {}\n",
        ])
        .assert()
        .failure();
}

/// `--content` and `--diff` are mutually exclusive: diff mode reads the
/// post-edit file from disk on purpose (it runs *after* the edit lands).
/// Mixing the two would silently throw away one of the inputs.
#[test]
fn content_conflicts_with_diff() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let cfg = write_trusted(root, no_panic_config());
    let patch = root.join("change.patch");
    std::fs::write(
        &patch,
        "--- a/src/foo.rs\n+++ b/src/foo.rs\n@@ -1 +1 @@\n-x\n+y\n",
    )
    .unwrap();
    let file = root.join("src/foo.rs");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(&file, "x\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--diff",
            patch.to_str().unwrap(),
            "--content",
            "y\n",
        ])
        .assert()
        .failure();
}

/// `--content` and `--session` are mutually exclusive: session mode
/// aggregates already-recorded edits, content mode evaluates one
/// pre-write payload. The two have no coherent intersection.
#[test]
fn content_conflicts_with_session() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let cfg = write_trusted(root, no_panic_config());
    let file = root.join("src/foo.rs");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(&file, "x\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--session",
            "--content",
            "y\n",
        ])
        .assert()
        .failure();
}

/// Empty `--content ""` is valid: an empty file is a legitimate
/// pre-write state (e.g., `write_file` creating a new empty file before
/// content is added). No rule fires on a literal-empty Rust source.
#[test]
fn content_empty_string_is_valid() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let file = root.join("src/foo.rs");
    std::fs::write(&file, "fn a() {}\n").unwrap();
    let cfg = write_trusted(root, no_panic_config());

    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--content",
            "",
            "--format",
            "json",
        ])
        .assert()
        .code(0);
}

/// File on disk does not exist, but `--content` provides the bytes — the
/// `write_file` case from spec §2. The path is used for scope matching
/// only; no disk read happens, so the missing file is fine.
#[test]
fn content_works_when_file_does_not_exist_on_disk() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    // Note: no fs::write — the file is purely virtual.
    let file = root.join("src/new.rs");
    let cfg = write_trusted(root, no_panic_config());

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--file",
            file.to_str().unwrap(),
            "--content",
            "fn b() { panic!(); }\n",
            "--format",
            "json",
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(parsed["status"], "block");
    assert_eq!(parsed["violations"][0]["rule_id"], "no-panic");
}
