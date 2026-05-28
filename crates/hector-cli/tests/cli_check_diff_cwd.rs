//! `hector check --diff` from an unrelated CWD must resolve diff target
//! paths against `config_dir`, not the process CWD. Otherwise `check_inner`
//! reads the file relative to the process CWD, gets empty content, and the
//! AST engine degrades to an `<rule>__internal` error violation.

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn ast_rule_fires_when_run_from_unrelated_cwd() {
    // Project lives in one tmpdir; the "agent" runs from a completely
    // different directory. The patch carries bare relative paths (the
    // normal `+++ b/src.rs` format).
    let proj = tempdir().unwrap();
    let other_cwd = tempdir().unwrap();

    // AST rule: block any use of `panic!` in Rust files.
    let cfg_body = "schema_version: 2\n\
                    rules:\n\
                    \x20\x20no-panic:\n\
                    \x20\x20\x20\x20description: no panics\n\
                    \x20\x20\x20\x20engine: ast\n\
                    \x20\x20\x20\x20language: rust\n\
                    \x20\x20\x20\x20scope: [\"**/*.rs\"]\n\
                    \x20\x20\x20\x20severity: error\n\
                    \x20\x20\x20\x20pattern: \"panic!($$$)\"\n";
    let cfg_path = proj.path().join(".hector.yml");
    fs::write(&cfg_path, cfg_body).unwrap();
    let signed =
        hector_core::trust::write_trust_block(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    fs::write(&cfg_path, signed).unwrap();

    // Source file in the project that the diff references with a bare
    // relative path (src.rs, not /tmp/A/src.rs).
    let src = proj.path().join("src.rs");
    fs::write(&src, "fn main() { panic!(\"oops\"); }\n").unwrap();

    // Patch lives in the *other* CWD, uses bare relative path.
    let patch = other_cwd.path().join("p.patch");
    fs::write(
        &patch,
        "--- a/src.rs\n\
         +++ b/src.rs\n\
         @@ -1,1 +1,1 @@\n\
         -fn main() {}\n\
         +fn main() { panic!(\"oops\"); }\n",
    )
    .unwrap();

    // Run from other_cwd — NOT from proj. hector must read
    // `config_dir/src.rs` = `proj/src.rs` and fire the real AST rule,
    // rather than reading `other_cwd/src.rs` and degrading to __internal.
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["check", "--diff"])
        .arg(&patch)
        .arg("--config")
        .arg(&cfg_path)
        .arg("--format")
        .arg("json")
        .current_dir(other_cwd.path())
        .output()
        .expect("run hector");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // Parse the JSON verdict so we're not sensitive to pretty-print spacing.
    let verdict: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("expected JSON stdout; got: {stdout}\nstderr: {stderr}\nerr: {e}")
    });

    let violations = verdict["violations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected violations array; got: {stdout}"));

    // The real rule must have fired.
    let rule_ids: Vec<&str> = violations
        .iter()
        .filter_map(|v| v["rule_id"].as_str())
        .collect();

    assert!(
        rule_ids.contains(&"no-panic"),
        "expected a real 'no-panic' violation; got rule_ids={rule_ids:?}\nstdout={stdout}\nstderr={stderr}"
    );

    // The __internal suffix (empty-content read) must not appear anywhere.
    assert!(
        !stdout.contains("__internal"),
        "must not produce __internal violation (indicates empty content read); stdout={stdout}\nstderr={stderr}"
    );

    // The violation is Error severity → Block → exit 2.
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2 (block); stdout={stdout}\nstderr={stderr}"
    );
}
