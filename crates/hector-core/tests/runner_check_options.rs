//! Runner-level coverage for CheckOptions: rule-id filter, explain capture,
//! and the prompt-render path that bypasses LLM dispatch.

use anyhow::Result;
use hector_core::config::Rule;
use hector_core::llm::{LlmClient, RuleStatus, RuleVerdict};
use hector_core::runner::{CheckInput, CheckOptions, HectorEngine};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::tempdir;

struct CountingLlm {
    calls: Arc<AtomicUsize>,
}

impl LlmClient for CountingLlm {
    fn evaluate(
        &self,
        rules: &[(&str, &Rule)],
        _primary: &str,
        _context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(rules
            .iter()
            .map(|(id, _)| RuleVerdict {
                rule_id: (*id).to_string(),
                status: RuleStatus::Pass,
            })
            .collect())
    }
}

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    std::fs::write(&path, body).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let with_trust = hector_core::trust::write_trust_block(&raw).unwrap();
    std::fs::write(&path, with_trust).unwrap();
    path
}

#[test]
fn explain_captures_every_in_scope_rule() {
    let dir = tempdir().unwrap();
    let body = "schema_version: 2\nrules:\n  pass-rule:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n  fire-rule:\n    description: \"y\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"exit 1\"\n  out-of-scope:\n    description: \"z\"\n    engine: script\n    scope: [\"*.rs\"]\n    severity: error\n    script: \"true\"\n";
    let cfg = write_trusted(dir.path(), body);
    let file = dir.path().join("foo.txt");
    std::fs::write(&file, "x\n").unwrap();

    let opts = CheckOptions {
        explain: true,
        ..CheckOptions::default()
    };
    let engine = HectorEngine::builder()
        .with_options(opts)
        .load(&cfg)
        .unwrap();
    let report = engine
        .check_with_explain(CheckInput::File {
            path: file.clone(),
            content: "x\n".to_string(),
        })
        .unwrap();
    assert_eq!(
        report.explain.len(),
        2,
        "only in-scope rules appear: {:?}",
        report.explain
    );
    let ids: Vec<&str> = report.explain.iter().map(|e| e.rule_id.as_str()).collect();
    assert!(ids.contains(&"pass-rule"));
    assert!(ids.contains(&"fire-rule"));
    assert!(!ids.contains(&"out-of-scope"));

    let fire = report
        .explain
        .iter()
        .find(|e| e.rule_id == "fire-rule")
        .unwrap();
    assert!(matches!(
        fire.outcome,
        hector_core::runner::ExplainOutcome::Fire
    ));
    let pass = report
        .explain
        .iter()
        .find(|e| e.rule_id == "pass-rule")
        .unwrap();
    assert!(matches!(
        pass.outcome,
        hector_core::runner::ExplainOutcome::Pass
    ));
}

#[test]
fn explain_off_leaves_explain_vec_empty() {
    let dir = tempdir().unwrap();
    let body = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n";
    let cfg = write_trusted(dir.path(), body);
    let file = dir.path().join("foo.txt");
    std::fs::write(&file, "x\n").unwrap();
    // Default options → explain=false → vec must be empty.
    let engine = HectorEngine::load(&cfg).unwrap();
    let report = engine
        .check_with_explain(CheckInput::File {
            path: file.clone(),
            content: "x\n".to_string(),
        })
        .unwrap();
    assert!(report.explain.is_empty());
    assert!(report.verdict.passed_checks.iter().any(|id| id == "r"));
}

#[test]
fn print_prompt_path_does_not_dispatch_llm() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  no-unwrap:\n    description: \"avoid unwrap\"\n    engine: semantic\n    scope: [\"**/*.rs\"]\n    severity: warning\n    context: diff\n",
    );
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main() { x.unwrap(); }\n").unwrap();
    let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,1 @@
-fn main() {}
+fn main() { x.unwrap(); }
";

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = HectorEngine::builder()
        .with_llm(Box::new(CountingLlm {
            calls: calls.clone(),
        }))
        .load(&cfg)
        .unwrap();
    let prompts = engine
        .render_semantic_prompts(CheckInput::Diff {
            file: file.clone(),
            unified_diff: diff.to_string(),
        })
        .unwrap();

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "render_semantic_prompts must not dispatch LLM"
    );
    assert_eq!(
        prompts.len(),
        1,
        "one in-scope semantic rule produces one prompt"
    );
    assert!(
        prompts[0].user.contains("unwrap"),
        "prompt user content includes diff"
    );
    assert!(
        prompts[0].system.contains("avoid unwrap"),
        "prompt system includes rule description"
    );
    assert_eq!(prompts[0].rule_id, "no-unwrap");
}

#[test]
fn render_semantic_prompts_skips_non_semantic_rules() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  ascr:\n    description: \"a\"\n    engine: script\n    scope: [\"*.rs\"]\n    severity: error\n    script: \"true\"\n  sem:\n    description: \"semantic check\"\n    engine: semantic\n    scope: [\"*.rs\"]\n    severity: warning\n    context: file\n",
    );
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main(){}\n").unwrap();
    let engine = HectorEngine::load(&cfg).unwrap();
    let prompts = engine
        .render_semantic_prompts(CheckInput::File {
            path: file.clone(),
            content: "fn main(){}\n".to_string(),
        })
        .unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].rule_id, "sem");
}

#[test]
fn render_semantic_prompts_honors_rule_filter() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  one:\n    description: \"first\"\n    engine: semantic\n    scope: [\"*.rs\"]\n    severity: warning\n    context: file\n  two:\n    description: \"second\"\n    engine: semantic\n    scope: [\"*.rs\"]\n    severity: warning\n    context: file\n",
    );
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main(){}\n").unwrap();
    let mut keep: HashSet<String> = HashSet::new();
    keep.insert("two".to_string());
    let opts = CheckOptions {
        rules: keep,
        ..CheckOptions::default()
    };
    let engine = HectorEngine::builder()
        .with_options(opts)
        .load(&cfg)
        .unwrap();
    let prompts = engine
        .render_semantic_prompts(CheckInput::File {
            path: file.clone(),
            content: "fn main(){}\n".to_string(),
        })
        .unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].rule_id, "two");
}

#[test]
fn rule_filter_runs_only_listed_ids() {
    let dir = tempdir().unwrap();
    let body = "schema_version: 2\nrules:\n  keep:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n  drop:\n    description: \"y\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"exit 1\"\n";
    let cfg = write_trusted(dir.path(), body);
    let file = dir.path().join("ok.txt");
    std::fs::write(&file, "clean\n").unwrap();

    let mut keep: HashSet<String> = HashSet::new();
    keep.insert("keep".to_string());
    let opts = CheckOptions {
        rules: keep,
        ..CheckOptions::default()
    };
    let engine = HectorEngine::builder()
        .with_options(opts)
        .load(&cfg)
        .unwrap();
    let verdict = engine
        .check(CheckInput::File {
            path: file.clone(),
            content: "clean\n".to_string(),
        })
        .unwrap();

    assert!(verdict.passed_checks.iter().any(|id| id == "keep"));
    assert!(!verdict.passed_checks.iter().any(|id| id == "drop"));
    assert!(verdict.violations.is_empty());
}

/// When a rule is filtered out via `CheckOptions.rules`, the runner must not
/// enter the parallel dispatch pool for it — and in particular must not
/// dispatch to the LLM.
#[test]
fn rule_filter_prevents_llm_dispatch_for_filtered_rules() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  keep:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n  drop-semantic:\n    description: \"y\"\n    engine: semantic\n    scope: [\"*.txt\"]\n    severity: warning\n    context: file\n",
    );
    let file = dir.path().join("ok.txt");
    std::fs::write(&file, "clean\n").unwrap();

    let mut keep: HashSet<String> = HashSet::new();
    keep.insert("keep".to_string());
    let opts = CheckOptions {
        rules: keep,
        ..CheckOptions::default()
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let engine = HectorEngine::builder()
        .with_options(opts)
        .with_llm(Box::new(CountingLlm {
            calls: calls.clone(),
        }))
        .load(&cfg)
        .unwrap();
    let _ = engine
        .check(CheckInput::File {
            path: file.clone(),
            content: "clean\n".to_string(),
        })
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "filtered-out semantic rule must not dispatch to the LLM"
    );
}
