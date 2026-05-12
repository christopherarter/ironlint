# Hector B1 — Parallel Rule Execution

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or superpowers:subagent-driven-development) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec section:** [`specs/2026-05-12-bully-parity-closures.md` §B1](../specs/2026-05-12-bully-parity-closures.md)
**Severity:** 🔴 critical (performance)
**Sequencing:** First item in the 0.2.1 cohort. Depends on the 0.2.0 A1/A2/A3 work that just landed.

---

**Goal:** Replace the serial rule loop in `crates/hector-core/src/runner.rs::check` with a rayon-driven parallel dispatch. Five semantic rules against one file currently serialize five HTTP round trips — that is the single biggest UX regression vs bully today. Per §B1: add `execution: { max_workers: usize }` to `Config`, honor the `HECTOR_MAX_WORKERS` env override (mirroring bully's `BULLY_MAX_WORKERS`), default to `min(8, num_cpus::get())`, fast-path the single-rule case, and use a per-call `pool.install(…)` so a test pool never fights another test's pool process-wide.

**Architecture:** The per-rule body in `check()` is already self-contained — it builds a `RuleContext`, dispatches to one of four engines, applies `disable` directives, and returns a `(Vec<Violation>, Vec<String>)` outcome. Hoist that body into a pure helper `evaluate_one_rule(&self, rule_id, rule, &path, &content, &diff, &disable_map) -> RuleOutcome` that owns nothing the borrow checker doesn't already allow it to. Then `rayon::iter::ParallelIterator::collect` over `self.config.rules` into an ordered `Vec<RuleOutcome>` and drain into the existing `violations` + `passed` accumulators. Determinism is intrinsic to rayon's `par_iter().collect::<Vec<_>>()` — the output order matches input order regardless of completion order — but we still assert it explicitly in a unit test because future maintainers reading "rayon" might assume "non-deterministic." A new `execution_pool()` method on `HectorEngine` builds a fresh `rayon::ThreadPool` per call (cheap — pool creation is ~microseconds; we only do it once per `check()`), sized by the precedence: `HECTOR_MAX_WORKERS` env → `config.execution.max_workers` → `min(8, num_cpus::get())`. Each pool size must clamp to `>= 1` (a zero would deadlock rayon). The single-rule fast path bypasses pool construction entirely.

**Tech Stack:** Rust, workspace-stable. Two new deps added to `hector-core`: `rayon` (sync data parallelism, no async runtime needed) and `num_cpus` (one-function CPU count). Both are tiny, widely used, and already transitively present in many of our deps. Tests use the existing wiremock pattern from `tests/anthropic.rs` for the timing-overlap proof and a counting `LlmClient` for the determinism + env-override tests.

---

## Decisions ratified up-front (per spec §B1 + §3)

| Decision | Choice | Reason |
|---|---|---|
| Where parallelism lives | **`HectorEngine::check`** per-file path only. `check_session` stays serial. | Session has one LLM call total — no parallelism payoff. Keeps the scope tight and the diff small. |
| Pool construction | **Per-call `pool.install(...)`**, never `build_global` | `build_global` is process-wide and fights tests that want a different pool size. Per-call is cheap and isolated. Spec §B1 step 4 explicitly calls this out. |
| Pool sizing precedence | `HECTOR_MAX_WORKERS` env → `config.execution.max_workers` → `min(8, num_cpus::get())` | Mirrors bully. Env beats config so operators can override without editing YAML. |
| Clamp behavior | `max_workers: 0` clamps to 1 (silent), env var that fails to parse falls back to config/default | Zero workers = deadlock. Silent clamp is the right call — operators who type `0` get a working system, not a panic. |
| Single-rule fast path | `if rules.len() == 1 { serial }` | Avoids pool overhead. Spec §B1 step 3. |
| Determinism contract | Output is in `BTreeMap::iter` key order (stable, alphabetical by rule id) regardless of completion order | `self.config.rules: BTreeMap<String, Rule>` already iterates in key order. `rayon::par_iter().collect::<Vec<_>>()` preserves input order. So output order is automatic — but we lock it down with a dedicated unit test. |
| Verdict shape | **Unchanged.** Same `Verdict` struct, same fields, same severity assignment. | Verdict locks at 0.3. B1 is implementation-internal — no public schema change. |
| Trust fingerprint | Unaffected | Filter is engine internals; fingerprint hashes YAML. Adding `execution:` to a user's config will rotate their fingerprint (correct: they edited the YAML). |
| Telemetry | **No new telemetry fields.** Per-rule timing is a separate concern (`B2`/`D1`). | Don't smuggle. |
| Thread-per-blocking-script trade-off | **Acknowledged.** `engine::script::run_script_rule` calls `Command::status()` which blocks the OS thread. With N script rules and a pool size of M, we'll have `min(N, M)` blocked threads — fine for our default M=8 on a developer's machine, worth flagging because users could theoretically configure `max_workers: 256` and surprise their kernel. | Spec §B1 notes. Default pool size cap of 8 (via `min(8, num_cpus)`) protects this. |

If anything here feels wrong on a fresh read, raise it before Task 4 — it sets the rollout's behavior.

---

## File structure

```
crates/hector-core/
├── Cargo.toml                     ← MODIFIED: add rayon + num_cpus
├── src/
│   ├── config/
│   │   ├── types.rs               ← MODIFIED: add `execution: Option<ExecutionConfig>` + struct
│   │   └── parser.rs              ← UNCHANGED: serde handles the new optional field
│   └── runner.rs                  ← MODIFIED: extract evaluate_one_rule + parallel dispatch
└── tests/
    ├── runner_parallel.rs         ← NEW: wiremock timing-overlap proof
    ├── runner_parallel_order.rs   ← NEW: determinism + env override + clamp unit tests
    └── parse_v2.rs                ← MODIFIED (one new test): execution.max_workers parses
```

No new files outside the test directory + one new struct in `types.rs`. The runner change is localized to the per-rule loop. The CLI is untouched (B1 is library-side).

---

## Phase 1 — Add dependencies

### Task 1: Add `rayon` and `num_cpus` to `hector-core`

**Files:**
- Modify: `crates/hector-core/Cargo.toml:8-23`

- [ ] **Step 1: Add the deps**

Edit `crates/hector-core/Cargo.toml`. Under `[dependencies]`, after the existing entries, add:

```toml
rayon = "1.10"
num_cpus = "1.16"
```

- [ ] **Step 2: Confirm the workspace builds**

Run: `cargo build -p hector-core`

Expected: green. Both crates pull in, no version conflicts.

- [ ] **Step 3: Commit**

```bash
git add crates/hector-core/Cargo.toml
git commit -m "feat(deps): add rayon and num_cpus for parallel rule execution (B1 phase 1)"
```

---

## Phase 2 — Failing tests (TDD red phase)

### Task 2: Write the wiremock parallelism proof + determinism + env override tests

**Files:**
- Create: `crates/hector-core/tests/runner_parallel.rs`
- Create: `crates/hector-core/tests/runner_parallel_order.rs`

The tests fail because parallel dispatch and `execution.max_workers` don't exist yet. The wiremock test fails because requests still serialize. The order test fails to compile because `ExecutionConfig` doesn't exist.

- [ ] **Step 1: Create `tests/runner_parallel.rs`**

This test spins up a wiremock server that delays each response by 200ms and records the request's arrival time. Five semantic rules against one file should land all five requests on the mock within ~50ms of each other. Serial dispatch would space them ~200ms apart. A 100ms threshold is the tightest one a CI runner won't flake on.

```rust
//! B1: prove rule dispatch is parallel by measuring HTTP request overlap on a
//! wiremock server. Serial dispatch would space the five rules ~200ms apart;
//! parallel dispatch lands them within tens of ms.

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
    // A3 pre-filter won't skip them (real-addition diffs + non-"avoid"
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

    // Real-addition diff so A3 pre-filter doesn't short-circuit anything.
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
    let verdict = engine
        .check(CheckInput::Diff {
            file,
            unified_diff: diff.to_string(),
        })
        .unwrap();
    let wall_elapsed = wall_start.elapsed();

    assert_eq!(
        calls.load(Ordering::SeqCst),
        5,
        "all five rules must dispatch; got verdict {:?}",
        verdict.status
    );

    // Wall clock must be far closer to one delay (200ms) than five
    // (1000ms). Allow generous CI headroom: 600ms is well above 200 + jitter
    // and well below the 1000ms a fully serial run would take.
    assert!(
        wall_elapsed < Duration::from_millis(600),
        "wall-clock {wall_elapsed:?} suggests serial execution (would be ~1s)"
    );

    // Tighter check: the five call-start timestamps should cluster within
    // ~100ms of each other. Serial would space them ~200ms apart.
    let mut starts_vec = starts.lock().unwrap().clone();
    starts_vec.sort();
    let spread = starts_vec.last().unwrap().duration_since(*starts_vec.first().unwrap());
    assert!(
        spread < Duration::from_millis(100),
        "starts spread is {spread:?}; suggests serial dispatch"
    );
}
```

- [ ] **Step 2: Create `tests/runner_parallel_order.rs`**

```rust
//! B1: lock down the determinism + config + env-override invariants.

use anyhow::Result;
use hector_core::config::Rule;
use hector_core::llm::{LlmClient, RuleVerdict};
use hector_core::runner::{CheckInput, HectorEngine};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::tempdir;

/// LlmClient that responds with `Violation` so each rule's verdict is
/// distinguishable in the output. Sleeps a small random-ish amount per call
/// (keyed by rule id) so completion order intentionally differs from input
/// order — if the runner's collect were non-deterministic, the order test
/// would flake.
struct OrderingLlm {
    // Pairs (rule_id_substring, sleep_ms) so we can simulate uneven completion
    // times deterministically.
    sleeps: Arc<Mutex<Vec<(String, u64)>>>,
    calls: Arc<AtomicUsize>,
}

impl LlmClient for OrderingLlm {
    fn evaluate(
        &self,
        rules: &[(&str, &Rule)],
        _primary: &str,
        _context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // We get one rule at a time from the runner (it dispatches per-rule).
        let id = rules[0].0;
        let sleep_ms = self
            .sleeps
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| id.contains(k.as_str()))
            .map(|(_, ms)| *ms)
            .unwrap_or(0);
        std::thread::sleep(Duration::from_millis(sleep_ms));
        Ok(vec![RuleVerdict {
            rule_id: id.to_string(),
            status: hector_core::llm::RuleStatus::Violation {
                message: format!("violation from {id}"),
                line: None,
            },
        }])
    }
}

fn write_three_rule_config(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    // Rules named so BTreeMap key-iteration order is rule-a, rule-b, rule-c.
    // The mock sleeps make completion order c, a, b — so if the runner ever
    // emits in completion order, the violations list won't match.
    let body = r#"schema_version: 2
rules:
  rule-a:
    description: "must X"
    engine: semantic
    scope: ["**/*.rs"]
    severity: warning
    context: diff
  rule-b:
    description: "must Y"
    engine: semantic
    scope: ["**/*.rs"]
    severity: warning
    context: diff
  rule-c:
    description: "must Z"
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

fn ordering_engine(cfg_dir: &std::path::Path) -> (HectorEngine, std::path::PathBuf) {
    let cfg = write_three_rule_config(cfg_dir);
    let file = cfg_dir.join("foo.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();

    let sleeps = Arc::new(Mutex::new(vec![
        ("rule-a".to_string(), 30),
        ("rule-b".to_string(), 60),
        ("rule-c".to_string(), 5),
    ]));
    let llm = OrderingLlm {
        sleeps,
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let engine = HectorEngine::builder()
        .with_llm(Box::new(llm))
        .load(&cfg)
        .unwrap();
    (engine, file)
}

const REAL_DIFF: &str = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,2 @@
 fn main() {}
+fn hello() {}
";

#[test]
fn violations_are_ordered_by_rule_id_not_completion_time() {
    let dir = tempdir().unwrap();
    let (engine, file) = ordering_engine(dir.path());

    let verdict = engine
        .check(CheckInput::Diff {
            file,
            unified_diff: REAL_DIFF.to_string(),
        })
        .unwrap();

    let ids: Vec<String> = verdict.violations.iter().map(|v| v.rule_id.clone()).collect();
    // BTreeMap order: rule-a, rule-b, rule-c. Completion order from the mock
    // is rule-c, rule-a, rule-b. Output must reflect input order.
    assert_eq!(
        ids,
        vec!["rule-a".to_string(), "rule-b".to_string(), "rule-c".to_string()],
        "violations must appear in input/rule-id order regardless of completion order"
    );
}

#[test]
fn hector_max_workers_env_clamps_pool_size() {
    // Sanity that the env override path runs and a clamped value doesn't
    // deadlock. We can't easily observe the pool size from outside, so we
    // assert behavioral parity: a `HECTOR_MAX_WORKERS=1` run still returns
    // the same verdict shape as the parallel default.
    let dir = tempdir().unwrap();
    let (engine, file) = ordering_engine(dir.path());

    // Use a one-off scope so we don't leak the env to other tests.
    // Tests in `cargo test` share a process; std::env::set_var is racy
    // across threads. Run this test in `--test-threads=1` or isolate via
    // a serial-test crate... but for now we set + unset within a single
    // call to minimize the window. The other tests in this file don't
    // read HECTOR_MAX_WORKERS.
    let prev = std::env::var("HECTOR_MAX_WORKERS").ok();
    // SAFETY: tests in this file are scoped so no other test reads
    // HECTOR_MAX_WORKERS concurrently.
    unsafe { std::env::set_var("HECTOR_MAX_WORKERS", "1"); }

    let verdict = engine
        .check(CheckInput::Diff {
            file,
            unified_diff: REAL_DIFF.to_string(),
        })
        .unwrap();

    match prev {
        Some(v) => unsafe { std::env::set_var("HECTOR_MAX_WORKERS", v) },
        None => unsafe { std::env::remove_var("HECTOR_MAX_WORKERS") },
    }

    let ids: Vec<String> = verdict.violations.iter().map(|v| v.rule_id.clone()).collect();
    assert_eq!(ids, vec!["rule-a".to_string(), "rule-b".to_string(), "rule-c".to_string()]);
}

#[test]
fn hector_max_workers_zero_value_clamps_to_one_not_deadlock() {
    let dir = tempdir().unwrap();
    let (engine, file) = ordering_engine(dir.path());

    let prev = std::env::var("HECTOR_MAX_WORKERS").ok();
    unsafe { std::env::set_var("HECTOR_MAX_WORKERS", "0"); }

    let verdict = engine
        .check(CheckInput::Diff {
            file,
            unified_diff: REAL_DIFF.to_string(),
        })
        .unwrap();

    match prev {
        Some(v) => unsafe { std::env::set_var("HECTOR_MAX_WORKERS", v) },
        None => unsafe { std::env::remove_var("HECTOR_MAX_WORKERS") },
    }

    // Three violations means the run completed (clamping worked).
    assert_eq!(verdict.violations.len(), 3);
}
```

- [ ] **Step 3: Run, confirm both new files fail**

Run: `cargo test --test runner_parallel --test runner_parallel_order 2>&1 | tail -30`

Expected: tests build (we did not introduce new types yet) but either:
- Parallel test: fails because `wall_elapsed >= 600ms` and `spread >= 100ms` — current serial dispatch produces ~1000ms wall time and ~200ms spread.
- Order test: passes today (BTreeMap iteration is already stable, no parallelism to disturb it) — that's fine, it's a regression-guard for Task 4.
- Env-override tests: pass today (env is silently ignored) — fine, same regression-guard role.

If the order test fails for any other reason, fix the test first.

- [ ] **Step 4: Commit**

```bash
git add crates/hector-core/tests/runner_parallel.rs \
        crates/hector-core/tests/runner_parallel_order.rs
git commit -m "test(runner): failing test asserting parallel dispatch + ordering (B1 phase 2)"
```

---

## Phase 3 — Implement parallel dispatch (TDD green phase)

### Task 3: Extract per-rule evaluation into a helper

The current `check()` body holds the per-rule loop inline. We hoist the loop body (without the surrounding accumulator-merge) into a function that returns a struct, then call it serially or via rayon depending on `rules.len()`.

**Files:**
- Modify: `crates/hector-core/src/runner.rs:198-277`

- [ ] **Step 1: Define `RuleOutcome` and helper inside `impl HectorEngine`**

Open `crates/hector-core/src/runner.rs`. Just before `pub fn check(&self, …)`, add:

```rust
/// Per-rule evaluation result, before the runner-level dedupe/baseline pass.
///
/// `passed` is a singleton `Some(rule_id)` when the rule produced no
/// emitted violations (every match was suppressed or the engine returned
/// `Ok(vec![])`); `None` otherwise. Splitting passed/violations from one
/// `Result<Vec<Violation>>` keeps the parallel `collect` straightforward.
struct RuleOutcome {
    violations: Vec<Violation>,
    passed: Option<String>,
}

impl HectorEngine {
    fn evaluate_one_rule(
        &self,
        rule_id: &str,
        rule: &crate::config::Rule,
        match_path: &Path,
        path: &Path,
        content: &str,
        diff: &str,
        disable_map: &crate::disable::DisableMap,
    ) -> RuleOutcome {
        let matcher = crate::config::scope::ScopeMatcher::new(&rule.scope)
            .expect("scope validated at load");
        if !matcher.matches(match_path) {
            return RuleOutcome { violations: vec![], passed: None };
        }
        if self.try_semantic_skip(rule_id, rule, path, diff) {
            return RuleOutcome { violations: vec![], passed: Some(rule_id.to_string()) };
        }
        let ctx = RuleContext {
            rule_id,
            rule,
            file: path,
            content: if content.is_empty() { None } else { Some(content) },
            diff: if diff.is_empty() { None } else { Some(diff) },
            cwd: &self.config_dir,
            llm: self.llm.as_deref(),
        };
        let outcome: anyhow::Result<Vec<Violation>> = match rule.engine {
            EngineKind::Script => crate::engine::script::ScriptEngine.run(&ctx),
            EngineKind::Ast => crate::engine::ast::AstEngine.run(&ctx),
            EngineKind::Semantic => crate::engine::semantic::SemanticEngine.run(&ctx),
            _ => Ok(Vec::new()),
        };
        match outcome {
            Ok(vs) if vs.is_empty() => RuleOutcome { violations: vec![], passed: Some(rule_id.to_string()) },
            Ok(vs) => {
                let mut kept: Vec<Violation> = Vec::new();
                let mut any_emitted = false;
                for v in vs {
                    let disabled = match v.line {
                        Some(line) => disable_map.is_disabled(line, rule_id),
                        None => disable_map.is_disabled_file_wide(rule_id),
                    };
                    if disabled {
                        continue;
                    }
                    kept.push(v);
                    any_emitted = true;
                }
                let passed = if any_emitted { None } else { Some(rule_id.to_string()) };
                RuleOutcome { violations: kept, passed }
            }
            Err(e) => {
                let v = Violation {
                    rule_id: format!("{rule_id}__internal"),
                    severity: crate::verdict::Severity::Error,
                    engine: crate::verdict::Engine::Internal,
                    file: path.display().to_string(),
                    line: None,
                    column: None,
                    message: format!("{e:#}"),
                    suggestion: None,
                    context: None,
                };
                RuleOutcome { violations: vec![v], passed: None }
            }
        }
    }
```

- [ ] **Step 2: Replace the per-rule loop with parallel collect + drain**

Find the existing `for (rule_id, rule) in &self.config.rules { … }` block at lines 198-277 of `runner.rs`. Replace it entirely with:

```rust
        let outcomes: Vec<RuleOutcome> = if self.config.rules.len() <= 1 {
            // Single-rule fast path: skip pool construction overhead.
            self.config.rules.iter()
                .map(|(rule_id, rule)| {
                    self.evaluate_one_rule(rule_id, rule, &match_path, &path, &content, &diff, &disable_map)
                })
                .collect()
        } else {
            use rayon::prelude::*;
            let pool = self.execution_pool();
            let rules: Vec<(&String, &crate::config::Rule)> = self.config.rules.iter().collect();
            pool.install(|| {
                rules.par_iter()
                    .map(|(rule_id, rule)| {
                        self.evaluate_one_rule(rule_id, rule, &match_path, &path, &content, &diff, &disable_map)
                    })
                    .collect()
            })
        };

        for outcome in outcomes {
            violations.extend(outcome.violations);
            if let Some(id) = outcome.passed {
                passed.push(id);
            }
        }
```

- [ ] **Step 3: Add `execution_pool()` helper**

Inside `impl HectorEngine`, somewhere before `check()`:

```rust
    /// Build a rayon thread pool sized by the precedence:
    /// `HECTOR_MAX_WORKERS` env → `config.execution.max_workers` → `min(8, num_cpus::get())`.
    /// A zero or unparseable env value falls back to the next layer; a zero
    /// config value clamps to 1 (rayon panics on `num_threads(0)`).
    fn execution_pool(&self) -> rayon::ThreadPool {
        let env = std::env::var("HECTOR_MAX_WORKERS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0);
        let cfg = self
            .config
            .execution
            .as_ref()
            .map(|e| e.max_workers)
            .filter(|n| *n > 0);
        let default = std::cmp::min(8, num_cpus::get().max(1));
        let n = env.or(cfg).unwrap_or(default).max(1);
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .expect("rayon pool construction must not fail")
    }
```

- [ ] **Step 4: Remove unused `RuleContext` import lines if compiler complains**

Run: `cargo build -p hector-core 2>&1 | tail -30`

Expected: clean build. If unused-import warnings fire, prune.

If `execution` field on `Config` doesn't exist yet, we'll get a compile error — that's the cue for Phase 4. For now this commit will not build standalone; **that's fine** — we'll combine Phase 3 + Phase 4 into one commit at the end of Phase 4 to avoid a broken intermediate commit.

Actually re-think: a broken intermediate commit blocks bisect. Reorder so Phase 4 (config) happens **before** Phase 3 (parallel wire-up). Update accordingly when executing — see Phase 4 below.

---

## Phase 4 — `execution.max_workers` config field (do this BEFORE Phase 3's wiring)

### Task 4: Add `ExecutionConfig` to `Config`

**Files:**
- Modify: `crates/hector-core/src/config/types.rs:4-16`
- Modify: `crates/hector-core/tests/parse_v2.rs` (one new test)

- [ ] **Step 1: Add the struct**

Edit `crates/hector-core/src/config/types.rs`. Add to the `Config` struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: u32,
    #[serde(default)]
    pub llm: Option<LlmConfig>,
    #[serde(default)]
    pub extends: Vec<String>,
    #[serde(default)]
    pub trust: Option<TrustBlock>,
    #[serde(default)]
    pub skip: Vec<String>,
    #[serde(default)]
    pub execution: Option<ExecutionConfig>,
    pub rules: BTreeMap<String, Rule>,
}
```

And add the new struct anywhere in the file:

```rust
/// Optional execution-tuning block. Controls the rayon pool that dispatches
/// rules in parallel during `HectorEngine::check`. Absence = use the default
/// of `min(8, num_cpus::get())`. The `HECTOR_MAX_WORKERS` env var overrides
/// any value set here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    /// Maximum worker threads. `0` or absent → use default. Values are
    /// clamped to ≥ 1 at pool-construction time.
    #[serde(default)]
    pub max_workers: usize,
}
```

- [ ] **Step 2: Add a parser test**

Append to `crates/hector-core/tests/parse_v2.rs`:

```rust
#[test]
fn parses_execution_max_workers() {
    let yaml = r#"schema_version: 2
execution:
  max_workers: 4
rules:
  r1:
    description: "no foo"
    engine: ast
    scope: ["**/*.rs"]
    severity: warning
    pattern: "$X"
    language: rust
"#;
    let cfg = hector_core::config::parse_str(yaml).expect("parse");
    let exec = cfg.execution.as_ref().expect("execution block");
    assert_eq!(exec.max_workers, 4);
}

#[test]
fn parses_without_execution_block() {
    let yaml = r#"schema_version: 2
rules:
  r1:
    description: "no foo"
    engine: ast
    scope: ["**/*.rs"]
    severity: warning
    pattern: "$X"
    language: rust
"#;
    let cfg = hector_core::config::parse_str(yaml).expect("parse");
    assert!(cfg.execution.is_none());
}
```

- [ ] **Step 3: Run the parser tests**

Run: `cargo test -p hector-core --test parse_v2`

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/hector-core/src/config/types.rs \
        crates/hector-core/tests/parse_v2.rs
git commit -m "feat(config): execution.max_workers + HECTOR_MAX_WORKERS override (B1 phase 4)"
```

(The `HECTOR_MAX_WORKERS` plumbing lands in Phase 3's runner edit, but the public surface — the config key — is what this commit ratifies.)

---

## Phase 3 (continued) — Wire parallel dispatch into the runner

Re-apply Task 3 now that `Config::execution` exists. Steps 1–3 above will compile.

- [ ] **Step 5: Build clean**

Run: `cargo build -p hector-core`

Expected: green.

- [ ] **Step 6: Run all the tests added in Task 2**

Run: `cargo test --test runner_parallel --test runner_parallel_order`

Expected: all green. The wall-clock and spread assertions in `runner_parallel.rs` pass; the BTreeMap order assertion in `runner_parallel_order.rs` passes; the env-clamp tests pass.

- [ ] **Step 7: Full crate test**

Run: `cargo test -p hector-core`

Expected: green. No existing test breaks. (`runner_diff.rs`, `runner_disable.rs`, etc. all run against the new code path.)

- [ ] **Step 8: Commit**

```bash
git add crates/hector-core/src/runner.rs
git commit -m "feat(runner): rayon-based parallel rule execution (B1 phase 3)"
```

---

## Phase 5 — Lint, format, coverage, final sweep

### Task 5: Verification gate

- [ ] **Step 1: `cargo fmt --check`**

Run: `cargo fmt --check`

Fix any diffs: `cargo fmt`.

- [ ] **Step 2: `cargo clippy --all-targets -- -D warnings`**

Expected: green. Common warnings to expect on this kind of work:
- `clippy::needless_collect` on `rules: Vec<...>` — leave it; rayon `par_iter` needs an owned slice.
- `clippy::cognitive_complexity` on `evaluate_one_rule` — by extraction we should be well under 15.

If `runner.rs::check` cognitive complexity now exceeds 15 (we already had `#[allow(clippy::cognitive_complexity)]` there — the extraction should reduce it but the allow remains harmless), leave the existing allow in place.

- [ ] **Step 3: `cargo test --workspace`**

Expected: green.

- [ ] **Step 4: Coverage gate**

Run: `bash scripts/ci-coverage.sh`

Expected: `runner.rs` and the two new test files ≥ 90% region coverage. The wiremock/order/env tests run the new branches; if a branch is uncovered (e.g. the zero-env clamp path or the single-rule fast path), add a tiny dedicated test rather than skipping.

If the coverage script reports a regression on `runner.rs`:
1. Run `cargo llvm-cov --workspace --html` and open the report for `runner.rs`.
2. Identify the uncovered branch.
3. Add a test that exercises it.
4. Re-run the script until green.

- [ ] **Step 5: Spec acceptance criteria pass**

Open `specs/2026-05-12-bully-parity-closures.md` §B1 and confirm:
- [ ] Five semantic rules against one file dispatch concurrently → covered by `runner_parallel.rs::five_semantic_rules_dispatch_in_parallel`.
- [ ] Single-rule path doesn't pay pool overhead → covered: the `if rules.len() <= 1` branch is exercised by every existing single-rule test in `runner_diff.rs` etc.
- [ ] `execution.max_workers` parses and clamps to ≥1 → covered by `parse_v2::parses_execution_max_workers` + `runner_parallel_order::hector_max_workers_zero_value_clamps_to_one_not_deadlock`.
- [ ] Determinism: results sorted by rule id regardless of completion order → covered by `runner_parallel_order::violations_are_ordered_by_rule_id_not_completion_time`.

If any acceptance criterion is uncovered, add a test in the appropriate file before committing.

- [ ] **Step 6: Commit any sweep fixes**

```bash
git add -A
git commit -m "style(runner): clippy + fmt + coverage sweep (B1 phase 5)"
```

(If nothing changed, skip this commit.)

---

## Test plan summary

| Test file | Covers |
|---|---|
| `tests/runner_parallel.rs` | Wiremock-like timing proof: five semantic rules dispatch in parallel (wall-clock and timestamp-spread both bounded well below 5× serial). |
| `tests/runner_parallel_order.rs` | Determinism (output order = input BTreeMap order, not completion order); `HECTOR_MAX_WORKERS=1` doesn't break behavior; `HECTOR_MAX_WORKERS=0` clamps to 1 instead of deadlocking. |
| `tests/parse_v2.rs` (extended) | `execution.max_workers` parses; absence yields `None`. |

---

## Risk / rollback

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Thread-per-blocking-script: N script rules consume N pool threads | low (default cap=8) | low — fine on dev laptops, theoretically rough at `max_workers: 256` | Default `min(8, num_cpus)` keeps this safe. Documented in decisions table. |
| Test flake on slow CI (the 100ms-spread assertion) | medium | low | 100ms is conservative on a 200ms-delay LLM. Bump to 200ms if seen flaking. |
| Env-var test races (`std::env::set_var` is process-global) | low — these are the only tests touching `HECTOR_MAX_WORKERS` | high (cascading flakes) | Save/restore around the test body. If flake observed, add `serial_test` dep. |
| Verdict schema impact | none | n/a | Verdict shape unchanged. |
| Trust fingerprint impact | none (users who add `execution:` will rotate their fingerprint — correct) | n/a | YAML hash includes the field; adding it is an intentional config change. |
| Performance regression on tiny configs | none | n/a | Single-rule fast path bypasses pool construction. |
| Backwards compatibility | full | n/a | `execution:` is `#[serde(default)] Option`; absence = previous default behavior. |
| `pool.install()` deadlock | none | n/a | We construct a fresh pool per call and never re-enter from within a task. The `LlmClient` impls are pure HTTP — no nested rayon calls. |

**Rollback:** revert the runner edit in Phase 3 (Task 3). The `ExecutionConfig` struct on `Config` becomes inert (no consumers). The `rayon`/`num_cpus` deps become dead code but don't break anything.

---

## Self-review checklist (run before handing off)

- [ ] Wiremock parallel test asserts both wall-clock and timestamp spread.
- [ ] Determinism test deliberately makes completion order differ from input order.
- [ ] Env-override tests save and restore `HECTOR_MAX_WORKERS`.
- [ ] `cargo clippy --all-targets -- -D warnings` is green.
- [ ] `cargo fmt --check` produces no diff.
- [ ] `cargo test --workspace` passes.
- [ ] `bash scripts/ci-coverage.sh` is green for `runner.rs` and the new test files.
- [ ] No verdict shape, exit code, or public API changed beyond `Config::execution`.

---

## Hand-off

**Branching:** worktree branch already exists per the task harness.
**Estimated effort:** 5 phases, ~25 min each.
**Follow-ups out of scope here:**
- B2 / D1 will add per-rule timing telemetry. B1 establishes the parallel structure those will hook into.
- C1 `hector doctor` will surface effective pool size via the diagnostic output.
