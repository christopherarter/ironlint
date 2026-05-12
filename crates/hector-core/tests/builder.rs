use anyhow::Result;
use hector_core::config::Rule;
use hector_core::llm::{LlmClient, RuleStatus, RuleVerdict};
use hector_core::runner::{CheckInput, HectorEngine};
use std::fs;
use tempfile::tempdir;

struct FakeLlm {
    canned: Vec<RuleVerdict>,
}

impl LlmClient for FakeLlm {
    fn evaluate(
        &self,
        _rules: &[(&str, &Rule)],
        _primary: &str,
        _context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>> {
        Ok(self.canned.clone())
    }
}

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    fs::write(&path, body).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    let with_trust = hector_core::trust::write_trust_block(&raw).unwrap();
    fs::write(&path, with_trust).unwrap();
    path
}

const SEMANTIC_RULE_CONFIG: &str = "schema_version: 2\nrules:\n  no-derived-state:\n    description: \"no derived state\"\n    engine: semantic\n    scope: [\"*.tsx\"]\n    severity: error\n    context: file\n";

#[test]
fn default_load_errors_on_semantic_rule_without_llm() {
    let dir = tempdir().unwrap();
    let path = write_trusted(dir.path(), SEMANTIC_RULE_CONFIG);
    let engine = HectorEngine::load(&path).expect("load");

    let file = dir.path().join("app.tsx");
    let content = "const X = () => null;\n";
    fs::write(&file, content).unwrap();

    let verdict = engine
        .check(CheckInput::File {
            path: file,
            content: content.into(),
        })
        .unwrap();

    assert_eq!(verdict.violations.len(), 1);
    let v = &verdict.violations[0];
    assert!(
        v.rule_id.ends_with("__internal"),
        "expected internal-error rule_id, got: {}",
        v.rule_id
    );
    assert!(
        v.message.contains("LlmClient") || v.message.contains("LLM client"),
        "expected an LLM-missing error message, got: {}",
        v.message
    );
}

#[test]
fn builder_with_fake_llm_uses_injected_client() {
    let dir = tempdir().unwrap();
    let path = write_trusted(dir.path(), SEMANTIC_RULE_CONFIG);
    let fake = FakeLlm {
        canned: vec![RuleVerdict {
            rule_id: "no-derived-state".to_string(),
            status: RuleStatus::Violation {
                message: "fake said no".into(),
                line: Some(7),
            },
        }],
    };
    let engine = HectorEngine::builder()
        .with_llm(Box::new(fake))
        .load(&path)
        .expect("load");

    let file = dir.path().join("app.tsx");
    let content = "const X = () => null;\n";
    fs::write(&file, content).unwrap();

    let verdict = engine
        .check(CheckInput::File {
            path: file,
            content: content.into(),
        })
        .unwrap();

    assert_eq!(verdict.violations.len(), 1);
    let v = &verdict.violations[0];
    assert_eq!(v.rule_id, "no-derived-state");
    assert_eq!(v.message, "fake said no");
    assert_eq!(v.engine, hector_core::verdict::Engine::Semantic);
}
