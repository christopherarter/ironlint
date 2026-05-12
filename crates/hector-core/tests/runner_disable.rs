use hector_core::runner::{CheckInput, HectorEngine};
use hector_core::verdict::Status;
use std::fs;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    fs::write(&path, body).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    let with_trust = hector_core::trust::write_trust_block(&raw).unwrap();
    fs::write(&path, with_trust).unwrap();
    path
}

const AST_RULE_CONFIG: &str = "schema_version: 2\nrules:\n  no-as-any:\n    description: \"avoid as any\"\n    engine: ast\n    scope: [\"*.ts\"]\n    severity: error\n    pattern: \"$E as any\"\n    language: TypeScript\n";

#[test]
fn ast_violation_without_disable_blocks() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.ts");
    let content = "const x = y as any;\n";
    fs::write(&file, content).unwrap();
    let cfg_path = write_trusted(dir.path(), AST_RULE_CONFIG);
    let engine = HectorEngine::load(&cfg_path).expect("load");
    let verdict = engine
        .check(CheckInput::File {
            path: file.clone(),
            content: content.into(),
        })
        .unwrap();
    assert_eq!(verdict.status, Status::Block);
    assert_eq!(verdict.violations.len(), 1);
    assert_eq!(verdict.violations[0].rule_id, "no-as-any");
    assert_eq!(verdict.violations[0].line, Some(1));
}

#[test]
fn ast_violation_with_disable_passes() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.ts");
    let content = "const x = y as any; // hector-disable: no-as-any reason: ok\n";
    fs::write(&file, content).unwrap();
    let cfg_path = write_trusted(dir.path(), AST_RULE_CONFIG);
    let engine = HectorEngine::load(&cfg_path).expect("load");
    let verdict = engine
        .check(CheckInput::File {
            path: file.clone(),
            content: content.into(),
        })
        .unwrap();
    assert_eq!(verdict.status, Status::Pass);
    assert!(verdict.violations.is_empty());
    assert!(verdict.passed_checks.iter().any(|r| r == "no-as-any"));
}
