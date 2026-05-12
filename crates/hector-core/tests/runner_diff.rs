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
