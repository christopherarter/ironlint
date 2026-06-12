//! Skip-pattern short-circuit at the top of HectorEngine::check.

use hector_core::runner::{CheckInput, HectorEngine};
use std::fs;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    fs::write(&path, body).unwrap();
    let trusted =
        hector_core::trust::write_trust_block(&fs::read_to_string(&path).unwrap()).unwrap();
    fs::write(&path, trusted).unwrap();
    path
}

#[test]
fn project_skip_list_is_honored() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        r#"schema_version: 2
skip:
  - "custom-ignore.txt"
rules:
  always-fire:
    description: "any"
    engine: script
    scope: ["*.txt"]
    severity: error
    script: "exit 1"
"#,
    );

    let engine = HectorEngine::load(&cfg).expect("load");
    let target = dir.path().join("custom-ignore.txt");
    fs::write(&target, "x\n").unwrap();
    let verdict = engine
        .check(CheckInput::File {
            path: target,
            content: "x\n".into(),
        })
        .expect("check");
    assert_eq!(verdict.status, hector_core::verdict::Status::Pass);
    assert!(verdict.violations.is_empty());
}

#[test]
#[ignore = "mutates global HOME — run with --ignored or under --test-threads=1"]
fn user_global_ignore_is_honored() {
    let dir = tempdir().unwrap();
    let fake_home = dir.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();
    fs::write(fake_home.join(".hector-ignore"), "*.special\n").unwrap();

    let cfg = write_trusted(
        dir.path(),
        r#"schema_version: 2
rules:
  always-fire:
    description: "any"
    engine: script
    scope: ["*.special"]
    severity: error
    script: "exit 1"
"#,
    );

    let target = dir.path().join("foo.special");
    fs::write(&target, "x\n").unwrap();

    let prev_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &fake_home);
    let engine = HectorEngine::load(&cfg).expect("load");
    if let Some(h) = prev_home {
        std::env::set_var("HOME", h);
    } else {
        std::env::remove_var("HOME");
    }

    let verdict = engine
        .check(CheckInput::File {
            path: target,
            content: "x\n".into(),
        })
        .expect("check");
    assert_eq!(verdict.status, hector_core::verdict::Status::Pass);
}
