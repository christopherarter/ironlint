//! A3 phase 4 — verify the semantic pre-filter short-circuits before any
//! LLM dispatch. We use an in-process `LlmClient` that counts every
//! `evaluate` invocation; `calls == 0` after a skipped check is the
//! functional equivalent of "no HTTP request reaches the mock" from the
//! spec's wiremock acceptance criterion.

use anyhow::Result;
use hector_core::config::Rule;
use hector_core::llm::{LlmClient, RuleStatus, RuleVerdict};
use hector_core::runner::{CheckInput, HectorEngine};
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
        // Should never be reached in the skip tests. Return a clean pass
        // for every requested rule so the dispatch test sees no
        // violations and no rule_id-mismatch error.
        Ok(rules
            .iter()
            .map(|(id, _)| RuleVerdict {
                rule_id: (*id).to_string(),
                status: RuleStatus::Pass,
            })
            .collect())
    }
}

fn write_trusted_config(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    let body = r#"schema_version: 2
rules:
  no-unwrap:
    description: "no unwrap in library code"
    engine: semantic
    scope:
      - "**/*.rs"
    severity: warning
    context: diff
"#;
    std::fs::write(&path, body).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let with_trust = hector_core::trust::write_trust_block(&raw).unwrap();
    std::fs::write(&path, with_trust).unwrap();
    path
}

#[test]
fn whitespace_only_diff_does_not_dispatch_llm() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted_config(dir.path());
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main() {}\n   \n").unwrap();

    let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,2 @@
 fn main() {}
+
";

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = HectorEngine::builder()
        .with_llm(Box::new(CountingLlm {
            calls: calls.clone(),
        }))
        .load(&cfg)
        .unwrap();

    let verdict = engine
        .check(CheckInput::Diff {
            file: file.clone(),
            unified_diff: diff.to_string(),
        })
        .unwrap();

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "LLM must not be invoked for whitespace-only diff"
    );
    assert!(
        verdict.passed_checks.iter().any(|id| id == "no-unwrap"),
        "skipped rule should land in passed_checks; got {:?}",
        verdict.passed_checks
    );
    assert!(verdict.violations.is_empty());
}

#[test]
fn real_addition_diff_dispatches_llm() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted_config(dir.path());
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main() {}\nfn hello() {}\n").unwrap();

    let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,2 @@
 fn main() {}
+fn hello() {}
";

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = HectorEngine::builder()
        .with_llm(Box::new(CountingLlm {
            calls: calls.clone(),
        }))
        .load(&cfg)
        .unwrap();

    let verdict = engine
        .check(CheckInput::Diff {
            file,
            unified_diff: diff.to_string(),
        })
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "LLM must be invoked once for a real code addition"
    );
    assert!(
        verdict.passed_checks.iter().any(|id| id == "no-unwrap"),
        "dispatched rule should pass; got {:?}",
        verdict.passed_checks
    );
}

#[test]
fn pure_deletion_against_avoid_rule_does_not_dispatch_llm() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted_config(dir.path());
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();

    let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,2 +1,1 @@
 fn main() {}
-fn dead() {}
";

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = HectorEngine::builder()
        .with_llm(Box::new(CountingLlm {
            calls: calls.clone(),
        }))
        .load(&cfg)
        .unwrap();

    let verdict = engine
        .check(CheckInput::Diff {
            file,
            unified_diff: diff.to_string(),
        })
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "LLM must not be invoked for pure-deletion against an 'avoid' rule"
    );
    assert!(
        verdict.passed_checks.iter().any(|id| id == "no-unwrap"),
        "skipped rule should land in passed_checks; got {:?}",
        verdict.passed_checks
    );
}

#[test]
fn semantic_skipped_telemetry_recorded() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted_config(dir.path());
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main() {}\n   \n").unwrap();

    let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,2 @@
 fn main() {}
+
";

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = HectorEngine::builder()
        .with_llm(Box::new(CountingLlm {
            calls: calls.clone(),
        }))
        .load(&cfg)
        .unwrap();
    let _ = engine
        .check(CheckInput::Diff {
            file,
            unified_diff: diff.to_string(),
        })
        .unwrap();

    let log = std::fs::read_to_string(dir.path().join(".hector/log.jsonl")).unwrap();
    // D1: typed shape with `type` discriminator.
    assert!(
        log.contains("\"type\":\"semantic_skipped\""),
        "telemetry missing semantic_skipped record; log was:\n{log}"
    );
    assert!(
        log.contains("\"reason\":\"whitespace_only\""),
        "telemetry missing whitespace_only reason; log was:\n{log}"
    );
    assert!(
        log.contains("\"rule\":\"no-unwrap\""),
        "rule field, not rule_id; log:\n{log}"
    );
}

#[test]
fn semantic_skipped_telemetry_uses_typed_variant() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted_config(dir.path());
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main() {}\n   \n").unwrap();

    let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,2 @@
 fn main() {}
+
";

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = HectorEngine::builder()
        .with_llm(Box::new(CountingLlm {
            calls: calls.clone(),
        }))
        .load(&cfg)
        .unwrap();
    engine
        .check(CheckInput::Diff {
            file,
            unified_diff: diff.to_string(),
        })
        .unwrap();

    let log = std::fs::read_to_string(dir.path().join(".hector/log.jsonl")).unwrap();
    // D1: typed shape with `type` discriminator.
    assert!(log.contains("\"type\":\"semantic_skipped\""), "log:\n{log}");
    assert!(
        log.contains("\"reason\":\"whitespace_only\""),
        "log:\n{log}"
    );
    assert!(
        log.contains("\"rule\":\"no-unwrap\""),
        "rule field, not rule_id; log:\n{log}"
    );
    // Per-rule check still recorded.
    assert!(
        log.contains("\"type\":\"check\""),
        "per-file Check record present; log:\n{log}"
    );
}

#[test]
fn engine_error_yields_per_rule_record_with_engine_error_reason() {
    // A semantic rule with no LLM client wired up: dispatch errors,
    // runner converts the error to an `Engine::Internal` violation, and
    // the per-rule record carries reason="engine_error".
    let dir = tempdir().unwrap();
    let cfg_body = r#"schema_version: 2
rules:
  needs-llm:
    description: "x"
    engine: semantic
    scope: ["*.rs"]
    severity: error
"#;
    let cfg_path = dir.path().join(".hector.yml");
    let trusted = hector_core::trust::write_trust_block(cfg_body).unwrap();
    std::fs::write(&cfg_path, trusted).unwrap();
    let target = dir.path().join("foo.rs");
    std::fs::write(&target, "fn main(){}\n").unwrap();

    // No `.with_llm(...)` — semantic engine will error.
    let engine = HectorEngine::load(&cfg_path).unwrap();
    let _ = engine
        .check(CheckInput::File {
            path: target,
            content: "fn main(){}\n".into(),
        })
        .unwrap();

    let entries = hector_core::telemetry::read_all(&dir.path().join(".hector/log.jsonl")).unwrap();
    let has_engine_error = entries.iter().any(|e| {
        matches!(
            e, hector_core::telemetry::LogEntry::Check { rules, .. }
            if rules.iter().any(|r| r.reason.as_deref() == Some("engine_error"))
        )
    });
    assert!(
        has_engine_error,
        "missing engine_error per-rule record; entries: {entries:#?}"
    );
}
