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

/// The built-in skip pattern for lock files (`Cargo.lock`, `yarn.lock`, …)
/// must suppress all rules before engine dispatch. Because the rule exits 1
/// (would fire if it ran), a clean Pass proves it never ran.
#[test]
fn cargo_lock_is_skipped_with_default_config() {
    let dir = tempdir().unwrap();
    // A script rule that would match Cargo.lock and produce a violation —
    // but the default skip pattern must suppress it before dispatch.
    let cfg = write_trusted(
        dir.path(),
        r#"schema_version: 2
rules:
  silly:
    description: "any"
    engine: script
    scope: ["**/*.lock"]
    severity: error
    script: "exit 1"
"#,
    );

    let engine = HectorEngine::load(&cfg).expect("load");
    let lockfile = dir.path().join("Cargo.lock");
    fs::write(&lockfile, "# generated\n").unwrap();
    let verdict = engine
        .check(CheckInput::File {
            path: lockfile.clone(),
            content: fs::read_to_string(&lockfile).unwrap(),
        })
        .expect("check");
    assert_eq!(verdict.status, hector_core::verdict::Status::Pass);
    assert!(verdict.violations.is_empty());
    assert!(
        verdict.passed_checks.is_empty(),
        "no rules should run — passed_checks must be empty"
    );

    let log = fs::read_to_string(dir.path().join(".hector/log.jsonl")).expect("telemetry");
    // A skip-pattern record folds into a Check with empty rules.
    assert!(
        log.contains("\"type\":\"check\"") && log.contains("\"rules\":[]"),
        "expected a typed check record with empty rules; log was:\n{log}"
    );
}

/// The telemetry record emitted for a skipped file must use the typed
/// `{"type":"check","rules":[]}` shape — no legacy `"kind":"skipped"` field.
#[test]
fn skip_pattern_emits_typed_check_with_empty_rules() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        r#"schema_version: 2
rules:
  silly:
    description: "any"
    engine: script
    scope: ["**/*.lock"]
    severity: error
    script: "exit 1"
"#,
    );

    let engine = hector_core::runner::HectorEngine::load(&cfg).expect("load");
    let lockfile = dir.path().join("Cargo.lock");
    fs::write(&lockfile, "# generated\n").unwrap();
    engine
        .check(hector_core::runner::CheckInput::File {
            path: lockfile.clone(),
            content: fs::read_to_string(&lockfile).unwrap(),
        })
        .expect("check");

    let log = fs::read_to_string(dir.path().join(".hector/log.jsonl")).expect("telemetry");
    // A skip-pattern record is a `Check` with empty `rules`.
    assert!(
        log.contains("\"type\":\"check\""),
        "telemetry must use typed shape; got:\n{log}"
    );
    assert!(
        log.contains("\"rules\":[]"),
        "skip-pattern record must have empty rules:\n{log}"
    );
    // No legacy `kind` field anywhere.
    assert!(
        !log.contains("\"kind\":\"skipped\""),
        "legacy `kind` must be gone:\n{log}"
    );
}
