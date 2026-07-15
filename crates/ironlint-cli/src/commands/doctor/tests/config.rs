use super::super::config::{
    check_config_parses, check_config_present, check_run_path, check_script_paths,
};
use super::super::Status;
use super::ctx_with;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn config_present_pass_when_file_exists() {
    let d = tempdir().unwrap();
    fs::write(d.path().join(".ironlint.yml"), "checks: {}\n").unwrap();
    let r = check_config_present(&ctx_with(d.path()));
    assert_eq!(r.status, Status::Pass);
}

#[test]
fn config_present_fail_when_file_missing() {
    let d = tempdir().unwrap();
    let r = check_config_present(&ctx_with(d.path()));
    assert_eq!(r.status, Status::Fail);
    assert!(r.remediation.unwrap().contains("ironlint init"));
}

#[test]
fn parses_fail_when_config_missing() {
    let d = tempdir().unwrap();
    let r = check_config_parses(&ctx_with(d.path()));
    assert_eq!(r.status, Status::Fail);
}

#[test]
fn parses_pass_on_valid_checks_config() {
    let d = tempdir().unwrap();
    fs::write(
        d.path().join(".ironlint.yml"),
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
    )
    .unwrap();
    let r = check_config_parses(&ctx_with(d.path()));
    assert_eq!(r.status, Status::Pass);
    assert!(r.detail.contains("1 check(s)"));
}

#[test]
fn check_scripts_warn_when_config_missing() {
    let d = tempdir().unwrap();
    let r = check_script_paths(&ctx_with(d.path()));
    assert_eq!(r.status, Status::Warn);
}

#[test]
fn check_scripts_pass_for_inline_commands() {
    let d = tempdir().unwrap();
    fs::write(
        d.path().join(".ironlint.yml"),
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
    )
    .unwrap();
    let r = check_script_paths(&ctx_with(d.path()));
    assert_eq!(r.status, Status::Pass);
}

#[test]
fn check_run_path_skips_inline_commands() {
    assert!(check_run_path(Path::new("."), "g", "grep -q TODO").is_none());
}

#[test]
fn check_run_path_skips_non_ironlint_paths() {
    assert!(check_run_path(Path::new("."), "g", "scripts/check.sh").is_none());
}

#[test]
fn check_run_path_fails_missing_script() {
    let d = tempdir().unwrap();
    let result = check_run_path(d.path(), "g", ".ironlint/scripts/missing.sh");
    assert!(result.is_some());
    assert!(result.unwrap().contains("not found"));
}

#[test]
fn check_run_path_skips_legacy_gates_path() {
    // After the rename, doctor only checks scripts under .ironlint/scripts/.
    // A legacy .ironlint/gates/ path is not the policy surface and is skipped
    // (returns None) rather than flagged as missing.
    assert!(check_run_path(Path::new("."), "g", ".ironlint/gates/missing.sh").is_none());
}
