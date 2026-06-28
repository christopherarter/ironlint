//! End-to-end coverage for `hector show-resolved-config` (checks model).
//!
//! Default output format (from show_resolved_config.rs) is TSV — one
//! tab-separated row per check:
//!   check_id<TAB>origin<TAB>files(comma-joined)<TAB>run

use assert_cmd::Command;
use tempfile::tempdir;

#[test]
fn show_resolved_config_default_tsv_row_per_check() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(
        &cfg,
        "checks:\n  no-todo:\n    files: [\"*.rs\", \"*.txt\"]\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
    )
    .unwrap();

    let out = Command::cargo_bin("hector")
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
        cols[1].contains(".hector.yml"),
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
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(
        &cfg,
        "checks:\n  alpha:\n    files: [\"*.rs\"]\n    run: \"true\"\n  beta:\n    files: [\"*.ts\"]\n    run: \"true\"\n",
    )
    .unwrap();

    let out = Command::cargo_bin("hector")
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
    let absent = dir.path().join(".hector.yml");
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["show-resolved-config", "--config", absent.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("ERROR: "),
        "stderr must lead with ERROR: prefix: {stderr}"
    );
}
