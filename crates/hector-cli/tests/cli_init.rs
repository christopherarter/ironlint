use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn init_scaffolds_for_rust_project() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"foo\"\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["init", "--dir", dir.path().to_str().unwrap()])
        .assert()
        .success();
    let cfg = fs::read_to_string(dir.path().join(".hector.yml")).unwrap();
    assert!(cfg.contains("schema_version: 2"));
    assert!(cfg.contains("rules:"));
}

#[test]
fn init_refuses_to_overwrite() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".hector.yml"), "existing\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["init", "--dir", dir.path().to_str().unwrap()])
        .assert()
        .failure();
}

/// P2-9 regression: `grep PATTERN {file} && exit 1 || exit 0` collapses
/// grep's exit-2 (regex/parse error) into exit 0 (pass), so a broken
/// rule silently passes forever. The fix routes exit codes through a
/// `case` statement so:
///   - 0 (found)        → exit 1 (violation)
///   - 1 (not found)    → exit 0 (pass)
///   - 2 (grep error)   → exit 2 (surfaced as violation by runner)
///
/// The template must not contain `|| exit 0` (the masking idiom) and
/// must contain `case $?` (the explicit exit-code routing).
#[test]
fn init_template_preserves_grep_error_exit_codes() {
    for (manifest, name, contents) in [
        ("Cargo.toml", "Cargo.toml", "[package]\nname = \"foo\"\n"),
        ("package.json", "package.json", "{}\n"),
        (
            "pyproject.toml",
            "pyproject.toml",
            "[project]\nname=\"x\"\n",
        ),
        // Generic stack — no manifest.
        ("", "", ""),
    ] {
        let dir = tempdir().unwrap();
        if !manifest.is_empty() {
            fs::write(dir.path().join(name), contents).unwrap();
        }
        Command::cargo_bin("hector")
            .unwrap()
            .args(["init", "--dir", dir.path().to_str().unwrap()])
            .assert()
            .success();
        let cfg = fs::read_to_string(dir.path().join(".hector.yml")).unwrap();

        // The Python template doesn't use grep; only assert on stacks that do.
        if manifest != "pyproject.toml" {
            assert!(
                !cfg.contains("|| exit 0"),
                "stack `{manifest}`: grep template must not mask exit 2 via `|| exit 0`; got:\n{cfg}"
            );
            assert!(
                cfg.contains("case $?"),
                "stack `{manifest}`: expected case-statement exit-code routing; got:\n{cfg}"
            );
        }
    }
}
