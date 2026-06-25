mod common;

use assert_cmd::Command;

fn cfg(dir: &std::path::Path, body: &str) {
    std::fs::write(dir.join(".hector.yml"), body).unwrap();
}

#[test]
fn exit_2_gate_blocks_and_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    cfg(
        dir.path(),
        "gates:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
    );
    let xdg = common::blessed_store(&dir.path().join(".hector.yml"));
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "// TODO\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--file",
            file.to_str().unwrap(),
            "--config",
            ".hector.yml",
        ])
        .assert()
        .code(2);
}

#[test]
fn clean_file_passes_and_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    cfg(
        dir.path(),
        "gates:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
    );
    let xdg = common::blessed_store(&dir.path().join(".hector.yml"));
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "// clean\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--file",
            file.to_str().unwrap(),
            "--config",
            ".hector.yml",
        ])
        .assert()
        .code(0);
}

#[test]
fn stdin_content_gates_prewrite() {
    let dir = tempfile::tempdir().unwrap();
    cfg(
        dir.path(),
        "gates:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
    );
    let xdg = common::blessed_store(&dir.path().join(".hector.yml"));
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "// clean\n").unwrap();
    let mut cmd = Command::cargo_bin("hector").unwrap();
    cmd.current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--file",
            file.to_str().unwrap(),
            "--content",
            "-",
            "--config",
            ".hector.yml",
        ]);
    cmd.write_stdin("// TODO later\n").assert().code(2);
}

#[test]
fn broken_gate_exits_3() {
    let dir = tempfile::tempdir().unwrap();
    cfg(
        dir.path(),
        "gates:\n  oops:\n    files: \"**/*.rs\"\n    run: \"no-such-binary-xyz\"\n",
    );
    let xdg = common::blessed_store(&dir.path().join(".hector.yml"));
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "x\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--file",
            file.to_str().unwrap(),
            "--config",
            ".hector.yml",
        ])
        .assert()
        .code(3);
}

#[test]
fn unknown_gate_filter_errors() {
    let dir = tempfile::tempdir().unwrap();
    cfg(
        dir.path(),
        "gates:\n  g:\n    files: \"*\"\n    run: \"true\"\n",
    );
    let xdg = common::blessed_store(&dir.path().join(".hector.yml"));
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "x\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--file",
            file.to_str().unwrap(),
            "--gate",
            "nope",
            "--config",
            ".hector.yml",
        ])
        .assert()
        .code(1);
}

#[test]
fn legacy_config_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    cfg(dir.path(), "schema_version: 2\nrules: {}\n");
    // Trust now rejects this unblessed config before the parser sees it; still exit 1.
    let xdg = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "x\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--file",
            file.to_str().unwrap(),
            "--config",
            ".hector.yml",
        ])
        .assert()
        .code(1);
}

#[test]
fn diff_mode_blocks_when_file_contains_todo() {
    let dir = tempfile::tempdir().unwrap();
    // Gate: grep for TODO; exit 2 on match.
    cfg(
        dir.path(),
        "gates:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"grep -q TODO \\\"$HECTOR_FILE\\\" && exit 2 || exit 0\"\n",
    );
    let xdg = common::blessed_store(&dir.path().join(".hector.yml"));
    // On-disk file contains TODO — gates read from disk, not from the diff.
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "// TODO fix this\n").unwrap();
    // Unified diff that references the file.
    let diff_content = "--- a/a.rs\n+++ b/a.rs\n@@ -1,1 +1,1 @@\n-// old\n+// TODO fix this\n";
    let diff_file = dir.path().join("changes.patch");
    std::fs::write(&diff_file, diff_content).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--diff",
            diff_file.to_str().unwrap(),
            "--config",
            ".hector.yml",
        ])
        .assert()
        .code(2);
}

#[test]
fn diff_mode_passes_when_file_is_clean() {
    let dir = tempfile::tempdir().unwrap();
    cfg(
        dir.path(),
        "gates:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"grep -q TODO \\\"$HECTOR_FILE\\\" && exit 2 || exit 0\"\n",
    );
    let xdg = common::blessed_store(&dir.path().join(".hector.yml"));
    // On-disk file is clean.
    let file = dir.path().join("b.rs");
    std::fs::write(&file, "// clean file\n").unwrap();
    let diff_content = "--- a/b.rs\n+++ b/b.rs\n@@ -1,1 +1,1 @@\n-// old\n+// clean file\n";
    let diff_file = dir.path().join("clean.patch");
    std::fs::write(&diff_file, diff_content).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--diff",
            diff_file.to_str().unwrap(),
            "--config",
            ".hector.yml",
        ])
        .assert()
        .code(0);
}

#[test]
fn gate_filter_runs_only_selected() {
    let dir = tempfile::tempdir().unwrap();
    cfg(
        dir.path(),
        "gates:\n  blocker:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n  passer:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    );
    let xdg = common::blessed_store(&dir.path().join(".hector.yml"));
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "x\n").unwrap();
    // Filtering to only `passer` means the blocker never runs -> exit 0.
    Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .args([
            "check",
            "--file",
            file.to_str().unwrap(),
            "--gate",
            "passer",
            "--config",
            ".hector.yml",
        ])
        .assert()
        .code(0);
}
