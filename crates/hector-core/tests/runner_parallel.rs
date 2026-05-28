//! Prove rule dispatch is parallel by measuring overlap between mock LLM
//! call timestamps. Serial dispatch would space the five rules ~200ms
//! apart; parallel dispatch lands them within tens of ms.

use anyhow::Result;
use hector_core::config::Rule;
use hector_core::llm::{LlmClient, RuleVerdict};
use hector_core::runner::{CheckInput, HectorEngine};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::tempdir;

/// LlmClient that sleeps for a fixed duration on every call, recording the
/// instant each call started so the test can measure overlap.
struct DelayingLlm {
    delay: Duration,
    starts: Arc<Mutex<Vec<Instant>>>,
    calls: Arc<AtomicUsize>,
}

impl LlmClient for DelayingLlm {
    fn evaluate(
        &self,
        rules: &[(&str, &Rule)],
        _primary: &str,
        _context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>> {
        self.starts.lock().unwrap().push(Instant::now());
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        Ok(rules
            .iter()
            .map(|(id, _)| RuleVerdict {
                rule_id: (*id).to_string(),
                status: hector_core::llm::RuleStatus::Pass,
            })
            .collect())
    }
}

fn write_trusted_five_rule_config(dir: &std::path::Path) -> std::path::PathBuf {
    // Five distinct semantic rules, all matching `*.rs`, all phrased so the
    // semantic pre-filter won't skip them (real-addition diff + non-"avoid"
    // descriptions).
    let path = dir.join(".hector.yml");
    let body = r#"schema_version: 2
rules:
  rule-a:
    description: "functions must have docs"
    engine: semantic
    scope: ["**/*.rs"]
    severity: warning
    context: diff
  rule-b:
    description: "functions must have docs B"
    engine: semantic
    scope: ["**/*.rs"]
    severity: warning
    context: diff
  rule-c:
    description: "functions must have docs C"
    engine: semantic
    scope: ["**/*.rs"]
    severity: warning
    context: diff
  rule-d:
    description: "functions must have docs D"
    engine: semantic
    scope: ["**/*.rs"]
    severity: warning
    context: diff
  rule-e:
    description: "functions must have docs E"
    engine: semantic
    scope: ["**/*.rs"]
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
fn five_semantic_rules_dispatch_in_parallel() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted_five_rule_config(dir.path());
    let file = dir.path().join("foo.rs");
    std::fs::write(&file, "fn main() {}\nfn hello() {}\n").unwrap();

    // Real-addition diff so the semantic pre-filter doesn't short-circuit.
    let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,2 @@
 fn main() {}
+fn hello() {}
";

    let starts = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let llm = DelayingLlm {
        delay: Duration::from_millis(200),
        starts: starts.clone(),
        calls: calls.clone(),
    };

    let engine = HectorEngine::builder()
        .with_llm(Box::new(llm))
        .load(&cfg)
        .unwrap();

    let wall_start = Instant::now();
    let _verdict = engine
        .check(CheckInput::Diff {
            file,
            unified_diff: diff.to_string(),
        })
        .unwrap();
    let wall_elapsed = wall_start.elapsed();

    assert_eq!(
        calls.load(Ordering::SeqCst),
        5,
        "all five rules must dispatch",
    );

    // Wall clock must be far closer to one delay (200ms) than five
    // (1000ms). Allow generous CI headroom: 600ms is well above 200 + jitter
    // and well below the 1000ms a fully serial run would take.
    assert!(
        wall_elapsed < Duration::from_millis(600),
        "wall-clock {wall_elapsed:?} suggests serial execution (would be ~1s)"
    );

    // Tighter check: the five call-start timestamps should cluster within
    // ~150ms of each other. Serial would space them ~200ms apart.
    let starts_vec = {
        let mut v = starts.lock().unwrap().clone();
        v.sort();
        v
    };
    let spread = starts_vec
        .last()
        .unwrap()
        .duration_since(*starts_vec.first().unwrap());
    assert!(
        spread < Duration::from_millis(150),
        "starts spread is {spread:?}; suggests serial dispatch (200ms+ would be serial)"
    );
}
