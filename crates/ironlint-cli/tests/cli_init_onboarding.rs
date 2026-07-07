use assert_cmd::Command;
use std::path::Path;

fn ironlint(home: &Path, project: &Path) -> Command {
    let mut c = Command::cargo_bin("ironlint").unwrap();
    c.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .current_dir(project);
    c
}

#[test]
fn init_installs_codex_hook_with_yes() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();

    ironlint(&home, &project)
        .args(["init", "--harness", "codex", "--yes"])
        .assert()
        .success();

    let hook = home.join(".config/ironlint/adapters/codex/hook.sh");
    assert!(hook.exists(), "hook artifact materialized");
    // No --global passed: init defaults to local (project) scope.
    let settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(project.join(".codex/hooks.json")).unwrap())
            .unwrap();
    assert!(settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("adapters/codex/hook.sh"));
}

#[test]
fn reinstall_reports_already_present() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    let run = || {
        ironlint(&home, &project)
            .args(["init", "--hook-only", "--harness", "codex", "--yes"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone()
    };
    run();
    let out = String::from_utf8(run()).unwrap();
    assert!(
        out.contains("already present"),
        "second run idempotent: {out}"
    );
}

#[test]
fn dry_run_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    ironlint(&home, &project)
        .args([
            "init",
            "--hook-only",
            "--harness",
            "codex",
            "--yes",
            "--dry-run",
        ])
        .assert()
        .success();
    // No --global passed: local scope, so the project-local settings file.
    assert!(!project.join(".codex/hooks.json").exists());
    assert!(!home
        .join(".config/ironlint/adapters/codex/hook.sh")
        .exists());
}

#[test]
fn uninstall_removes_hook() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    ironlint(&home, &project)
        .args(["init", "--hook-only", "--harness", "codex", "--yes"])
        .assert()
        .success();
    ironlint(&home, &project)
        .args(["init", "--uninstall", "--harness", "codex"])
        .assert()
        .success();
    assert!(!home
        .join(".config/ironlint/adapters/codex/hook.sh")
        .exists());
    // No --global passed on either invocation: local (project) scope.
    let settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(project.join(".codex/hooks.json")).unwrap())
            .unwrap();
    let arr = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert!(
        arr.iter().all(|e| {
            e["hooks"].as_array().into_iter().flatten().all(|h| {
                !h["command"]
                    .as_str()
                    .unwrap_or("")
                    .contains("adapters/codex/hook.sh")
            })
        }),
        "uninstall must remove the ironlint PreToolUse entry"
    );
}

#[test]
fn no_tty_without_yes_or_harness_skips_hooks() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();

    // assert_cmd pipes stdin (non-TTY); bare init must not install.
    let out = ironlint(&home, &project)
        .args(["init", "--hook-only"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        String::from_utf8(out).unwrap().contains("re-run with"),
        "non-TTY path must print the re-run hint"
    );
    // Auto-detect (no --harness) stops before install, so neither the
    // local (project) nor global settings file is ever written.
    assert!(!project.join(".codex/hooks.json").exists());
}

#[test]
fn explicit_harness_renders_plan_with_requested_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    let out = ironlint(&home, &project)
        .args(["init", "--hook-only", "--harness", "codex", "--yes"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("ironlint · onboarding"), "header:\n{s}");
    assert!(s.contains("codex"), "harness:\n{s}");
    assert!(s.contains("requested"), "explicit → requested tag:\n{s}");
    assert!(s.contains("hook"), "hook step listed:\n{s}");
    // --yes still installs
    assert!(home
        .join(".config/ironlint/adapters/codex/hook.sh")
        .exists());
}

#[test]
fn dry_run_renders_plan_but_installs_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    let out = ironlint(&home, &project)
        .args(["init", "--hook-only", "--harness", "codex", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    assert!(
        s.contains("ironlint · onboarding"),
        "dry-run still renders plan:\n{s}"
    );
    assert!(
        !home
            .join(".config/ironlint/adapters/codex/hook.sh")
            .exists(),
        "dry-run writes nothing"
    );
}

#[test]
fn uninstall_renders_removal_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    ironlint(&home, &project)
        .args(["init", "--hook-only", "--harness", "codex", "--yes"])
        .assert()
        .success();
    let out = ironlint(&home, &project)
        .args(["init", "--uninstall", "--harness", "codex", "--yes"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("ironlint · uninstall"), "uninstall header:\n{s}");
    assert!(
        !home
            .join(".config/ironlint/adapters/codex/hook.sh")
            .exists(),
        "uninstall removes the hook"
    );
}

#[test]
fn yes_bypasses_toggle_and_installs_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&home.join(".codex")).unwrap();

    let out = ironlint(&home, &project)
        .args(["init", "--hook-only", "--yes"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    assert!(
        !s.contains("Select harnesses"),
        "--yes must bypass the multi-select UI:\n{s}"
    );
    assert!(
        home.join(".config/ironlint/adapters/codex/hook.sh").exists(),
        "codex hook should be installed when detected"
    );
}

#[test]
fn explicit_harness_with_yes_skips_toggle_and_installs() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    let out = ironlint(&home, &project)
        .args(["init", "--hook-only", "--harness", "codex", "--yes"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    assert!(
        !s.contains("Select harnesses"),
        "explicit --harness must skip the multi-select UI:\n{s}"
    );
    assert!(
        home.join(".config/ironlint/adapters/codex/hook.sh").exists(),
        "codex hook should be installed"
    );
}

#[test]
fn dry_run_does_not_enter_toggle() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    let out = ironlint(&home, &project)
        .args(["init", "--hook-only", "--harness", "codex", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    assert!(
        !s.contains("Select harnesses"),
        "dry-run must not enter the multi-select UI:\n{s}"
    );
    assert!(
        !home
            .join(".config/ironlint/adapters/codex/hook.sh")
            .exists(),
        "dry-run must not install anything"
    );
    assert!(!project.join(".codex/hooks.json").exists(), "dry-run writes nothing");
}
