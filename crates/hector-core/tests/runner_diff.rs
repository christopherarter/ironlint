use hector_core::runner::{CheckInput, HectorEngine};
use hector_core::verdict::Status;
use tempfile::tempdir;

fn write_trusted_config(dir: &std::path::Path, config_body: &str) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    std::fs::write(&path, config_body).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let with_trust = hector_core::trust::write_trust_block(&raw).unwrap();
    std::fs::write(&path, with_trust).unwrap();
    path
}

const DIFF_BODY: &str = "\
--- a/src/foo.ts
+++ b/src/foo.ts
@@ -1,3 +1,4 @@
 line one
-old line
+new line
+added line
 line three
";

#[test]
fn runner_accepts_diff_input_with_passing_rule() {
    let dir = tempdir().unwrap();
    let cfg = "schema_version: 2\nrules:\n  noop:\n    description: \"x\"\n    engine: script\n    scope: [\"*.ts\"]\n    severity: error\n    script: \"true\"\n";
    let cfg_path = write_trusted_config(dir.path(), cfg);
    let engine = HectorEngine::load(&cfg_path).expect("load");
    let verdict = engine
        .check(CheckInput::Diff {
            file: std::path::PathBuf::from("src/foo.ts"),
            unified_diff: DIFF_BODY.to_string(),
        })
        .unwrap();
    assert_eq!(verdict.status, Status::Pass);
    assert!(verdict.passed_checks.contains(&"noop".to_string()));
}

#[test]
fn runner_accepts_diff_input_with_blocking_rule() {
    let dir = tempdir().unwrap();
    let cfg = "schema_version: 2\nrules:\n  always-fail:\n    description: \"x\"\n    engine: script\n    scope: [\"*.ts\"]\n    severity: error\n    script: \"exit 1\"\n";
    let cfg_path = write_trusted_config(dir.path(), cfg);
    let engine = HectorEngine::load(&cfg_path).expect("load");
    let verdict = engine
        .check(CheckInput::Diff {
            file: std::path::PathBuf::from("src/foo.ts"),
            unified_diff: DIFF_BODY.to_string(),
        })
        .unwrap();
    assert_eq!(verdict.status, Status::Block);
    assert_eq!(verdict.violations.len(), 1);
    assert_eq!(verdict.violations[0].rule_id, "always-fail");
}

// In diff mode, AST rules must run against the post-edit file on disk, not
// see an empty `content` and fail with "requires file content".
#[test]
fn ast_rule_runs_in_diff_mode_when_file_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let target = root.join("src/foo.rs");
    std::fs::write(&target, "fn main() { let _ = foo.unwrap(); }\n").unwrap();
    let cfg = "schema_version: 2\nrules:\n  no-unwrap:\n    description: x\n    engine: ast\n    language: rust\n    scope: [\"src/**/*.rs\"]\n    severity: warning\n    pattern: $E.unwrap()\n";
    let cfg_path = write_trusted_config(root, cfg);
    let engine = HectorEngine::load(&cfg_path).unwrap();
    let diff =
        "--- a/src/foo.rs\n+++ b/src/foo.rs\n@@ -1 +1 @@\n-fn main() {}\n+fn main() { let _ = foo.unwrap(); }\n";
    let v = engine
        .check(CheckInput::Diff {
            file: target.clone(),
            unified_diff: diff.to_string(),
        })
        .unwrap();
    // The rule must run against on-disk content and produce a real Warn, not
    // a Block with an `__internal` error about missing file content.
    assert!(
        v.violations.iter().any(|x| x.rule_id == "no-unwrap"),
        "ast rule must fire in diff mode; got {v:?}"
    );
    assert!(
        v.violations
            .iter()
            .all(|x| !x.rule_id.ends_with("__internal")),
        "no internal-error violations expected; got {v:?}"
    );
}

// `hector-disable` directives are read from the file source; in diff mode the
// runner must read the file from disk so those directives still apply.
#[test]
fn hector_disable_directive_applies_in_diff_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let target = root.join("src/foo.rs");
    std::fs::write(
        &target,
        "fn main() { let _ = foo.unwrap(); } // hector-disable: no-unwrap\n",
    )
    .unwrap();
    let cfg = "schema_version: 2\nrules:\n  no-unwrap:\n    description: x\n    engine: ast\n    language: rust\n    scope: [\"src/**/*.rs\"]\n    severity: error\n    pattern: $E.unwrap()\n";
    let cfg_path = write_trusted_config(root, cfg);
    let engine = HectorEngine::load(&cfg_path).unwrap();
    let diff = "--- a/src/foo.rs\n+++ b/src/foo.rs\n@@ -1 +1 @@\n-x\n+fn main() { let _ = foo.unwrap(); } // hector-disable: no-unwrap\n";
    let v = engine
        .check(CheckInput::Diff {
            file: target,
            unified_diff: diff.to_string(),
        })
        .unwrap();
    assert!(
        !v.violations.iter().any(|x| x.rule_id == "no-unwrap"),
        "hector-disable on the same line must silence the rule, got {v:?}"
    );
    // The rule must be actually evaluated and silenced — no `__internal`
    // error either, which would mean the engine never ran against real content.
    assert!(
        v.violations
            .iter()
            .all(|x| !x.rule_id.ends_with("__internal")),
        "no internal-error violations expected; got {v:?}"
    );
}
