//! End-to-end coverage for `ironlint show-resolved-config` (checks model).
//!
//! Default output format (from show_resolved_config.rs) is TSV — one
//! tab-separated row per check:
//!   check_id<TAB>origin<TAB>files(comma-joined)<TAB>run

use assert_cmd::Command;
use tempfile::tempdir;

fn write_config(path: &std::path::Path, content: &str) {
    std::fs::write(path, content).unwrap();
}

#[test]
fn show_resolved_config_default_tsv_row_per_check() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    std::fs::write(
        &cfg,
        "checks:\n  no-todo:\n    files: [\"*.rs\", \"*.txt\"]\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
    )
    .unwrap();

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["show-resolved-config", "--config", cfg.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    let line = stdout
        .lines()
        .find(|l| l.starts_with("no-todo"))
        .expect("default format must emit a TSV row for no-todo");
    let cols: Vec<&str> = line.split('\t').collect();
    assert_eq!(
        cols.len(),
        4,
        "TSV row must be 4 tab-separated columns: {line:?}"
    );
    assert_eq!(cols[0], "no-todo", "col 1 is the check id");
    assert!(
        cols[1].contains(".ironlint.yml"),
        "col 2 (origin) must reference the config file: {line:?}"
    );
    assert_eq!(
        cols[2], "*.rs,*.txt",
        "col 3 is the comma-joined files glob: {line:?}"
    );
    assert!(
        cols[3].contains("grep -q TODO"),
        "col 4 is the run command: {line:?}"
    );
}

#[test]
fn show_resolved_config_lists_multiple_checks() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    std::fs::write(
        &cfg,
        "checks:\n  alpha:\n    files: [\"*.rs\"]\n    run: \"true\"\n  beta:\n    files: [\"*.ts\"]\n    run: \"true\"\n",
    )
    .unwrap();

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["show-resolved-config", "--config", cfg.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    assert!(stdout.contains("alpha"), "must show alpha check: {stdout}");
    assert!(stdout.contains("beta"), "must show beta check: {stdout}");
}

#[test]
fn show_resolved_config_missing_config_exits_one() {
    let dir = tempdir().unwrap();
    let absent = dir.path().join(".ironlint.yml");
    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["show-resolved-config", "--config", absent.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("error: "),
        "stderr must lead with error: prefix: {stderr}"
    );
}

#[test]
fn show_resolved_config_lowers_architecture_block_into_arch_check() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    write_config(
        &cfg,
        "architecture:\n  layers:\n    - name: data\n      globs: [\"src/data/**\"]\n  rules:\n    - from: data\n      may_import: []\nchecks: {}\n",
    );

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["show-resolved-config", "--config", cfg.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    assert!(
        stdout.contains("__arch__"),
        "must show synthetic __arch__ check: {stdout}"
    );
    let line = stdout
        .lines()
        .find(|l| l.starts_with("__arch__"))
        .expect("__arch__ row must exist");
    let cols: Vec<&str> = line.split('\t').collect();
    assert_eq!(cols.len(), 4, "TSV row must be 4 columns: {line:?}");
    assert_eq!(cols[0], "__arch__");
    assert_eq!(cols[1], "<architecture>", "synthetic check origin label");
    assert_eq!(cols[2], "**/*", "__arch__ matches all files");
    assert!(
        cols[3].contains("ironlint arch check"),
        "__arch__ run must shell out to ironlint arch check: {line:?}"
    );
}

#[test]
fn show_resolved_config_lowers_inherited_architecture_block() {
    let dir = tempdir().unwrap();
    let base = dir.path().join("base.yml");
    let cfg = dir.path().join(".ironlint.yml");
    write_config(
        &base,
        "checks: {}\narchitecture:\n  layers:\n    - name: data\n      globs: [\"src/data/**\"]\n  rules:\n    - from: data\n      may_import: []\n",
    );
    write_config(
        &cfg,
        "extends: [\"base.yml\"]\nchecks:\n  other:\n    files: \"*.rs\"\n    run: \"true\"\n",
    );

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["show-resolved-config", "--config", cfg.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    assert!(
        stdout.contains("__arch__"),
        "must show synthetic __arch__ check inherited from base: {stdout}"
    );
    assert!(
        stdout.contains("other"),
        "must still show local check: {stdout}"
    );
    let arch_line = stdout
        .lines()
        .find(|l| l.starts_with("__arch__"))
        .expect("__arch__ row must exist");
    let cols: Vec<&str> = arch_line.split('\t').collect();
    assert_eq!(cols[1], "<architecture>");
}

#[test]
fn show_resolved_config_architecture_validation_error_exits_one() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    // Invalid: no layers.
    write_config(&cfg, "architecture:\n  layers: []\nchecks: {}\n");

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["show-resolved-config", "--config", cfg.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "invalid architecture must exit 1"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("error: "),
        "stderr must lead with error: prefix: {stderr}"
    );
}

#[test]
fn show_resolved_config_architecture_reserved_check_id_exits_one() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    write_config(
        &cfg,
        "architecture:\n  layers:\n    - name: data\n      globs: [\"src/data/**\"]\nchecks:\n  __arch__:\n    files: \"*\"\n    run: \"true\"\n",
    );

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["show-resolved-config", "--config", cfg.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "reserved __arch__ check id must exit 1"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("error: "),
        "stderr must lead with error: prefix: {stderr}"
    );
}

#[test]
fn show_resolved_config_architecture_in_yaml_format() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    write_config(
        &cfg,
        "architecture:\n  layers:\n    - name: data\n      globs: [\"src/data/**\"]\nchecks: {}\n",
    );

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "show-resolved-config",
            "--config",
            cfg.to_str().unwrap(),
            "--format",
            "yaml",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    assert!(
        stdout.contains("__arch__"),
        "yaml output must include __arch__: {stdout}"
    );
    assert!(
        stdout.contains("<architecture>"),
        "yaml output must include synthetic origin: {stdout}"
    );
}

#[test]
fn show_resolved_config_architecture_in_json_format() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".ironlint.yml");
    write_config(
        &cfg,
        "architecture:\n  layers:\n    - name: data\n      globs: [\"src/data/**\"]\nchecks: {}\n",
    );

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args([
            "show-resolved-config",
            "--config",
            cfg.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    assert!(
        stdout.contains("__arch__"),
        "json output must include __arch__: {stdout}"
    );
    assert!(
        stdout.contains("<architecture>"),
        "json output must include synthetic origin: {stdout}"
    );
}
