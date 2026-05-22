# Hector H1 — `hector check --emit-semantic-payload`

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec section:** [`specs/2026-05-14-subagent-semantic-eval.md` §H1](../specs/2026-05-14-subagent-semantic-eval.md)
**Severity:** 🔴 critical (scaffolding for the subscription-mediated semantic-eval feature; H3 adapter mode depends on this + H2 shipping)
**Sequencing:** 0.2.x cohort, independent of H2 — both can ship in parallel; H3 (`adapters/claude-code/`) is the consumer that needs them.

---

**Goal:** Teach the `hector check` CLI to run deterministic engines (script + ast), collect every `engine: semantic` and `engine: session` rule that would have dispatched, and emit a single `DeferredVerdict` JSON envelope wrapping the bully-shaped payload Claude Code's `additionalContext` consumes. Activated either by a new `llm.provider: claude-code-subagent` value in `.hector.yml` or by the long-only `--emit-semantic-payload` flag. The flag is the user-invisible internal switch the adapter uses in tests and scripted invocations; the config value is what end-users set. Direct-API users (anthropic / openrouter / ollama / API-key paths) are untouched.

**Architecture:** Add a `DeferredVerdict` type next to `Verdict` (new module `crates/hector-core/src/verdict_deferred.rs`, additive, never replaces the locked `Verdict` shape). Add a `emit_semantic_payload: bool` field on `CheckOptions` and route it through `HectorEngine::check_with_explain`: when active, the runner short-circuits `engine: semantic` and `engine: session` rules into a `Vec<DeferredRule>` instead of dispatching them, then `CheckReport` gains a `Option<DeferredVerdict>` field that the CLI inspects. The runner change is local to two dispatch arms in `runner.rs`; the rest of the pipeline (script, ast, baseline, telemetry of deterministic outcomes, exit-code computation for deterministic blocks) is untouched. The CLI computes the final exit code by inspecting both the deterministic verdict and whether a deferred envelope is present: deterministic block → exit 2, deterministic pass + deferred → emit `DeferredVerdict` and exit 0, deterministic pass + no deferred → emit standard `Verdict` and exit 0. `--emit-semantic-payload` is mutually exclusive with `--session` (clap-enforced); `--print-prompt` and `--explain` continue to short-circuit before the deferred path runs.

**Tech Stack:** Rust, workspace-stable. No new runtime dependencies. clap (existing) for the new flag, `serde` + `serde_json` (workspace) for the new envelope, `assert_cmd` + `tempfile` + `insta` (existing dev-deps) for the CLI integration test and snapshot. `chrono` is already in-tree for the rfc3339 timestamp on the existing `passed_checks` records.

---

## Decisions ratified up-front (per spec §4 and §6)

| Decision | Choice | Reason / source |
|---|---|---|
| Mode selection | New value `provider: claude-code-subagent` in `.hector.yml`'s `llm:` block. The new `--emit-semantic-payload` long flag forces the deferred path regardless of provider (used by tests and explicit adapter invocations). | Spec §4 — explicit, visible in config diffs, covered by the trust fingerprint. |
| Provider arm `Ok(None)` semantics | The `claude-code-subagent` arm returns `Ok(None)` **without** the existing missing-API-key stderr warning. The runner detects this and emits the deferred payload rather than treating semantic rules as "LLM unavailable". | Spec §H1 step 1. The missing-key path is "user misconfigured"; the subagent path is "user opted out of direct dispatch". Different UX. |
| Session-rule treatment | `engine: session` rules are deferred under the same flag, joining `evaluate` in the payload distinguished by a per-rule `engine` field (`"semantic"` \| `"session"`). | Spec §6 Q1, default proposed. The adapter's interpreter skill already routes both through the subagent — keeping them in one payload mirrors bully and avoids two adapter codepaths. |
| Config-vs-env tie-break | If `llm.provider: claude-code-subagent` is set **and** `ANTHROPIC_API_KEY` is exported, the config wins. The runner emits the deferred payload; the env var is ignored for routing (it's still readable for unrelated calls). | Spec §6 Q4, default proposed. The explicit config value is the operator's signal; env-var presence is incidental. |
| `--emit-semantic-payload` flag visibility | Visible in `--help` (consistent with `--print-prompt`). Documented as adapter-internal in `docs/`. | Spec §H1 step 2 calls it "adapter-internal" but does not require hiding. Hidden flags rot. |
| Mutual exclusion with `--session` | clap-enforced (`conflicts_with = "session"`). Reasoning: `--session` evaluates the cumulative changeset; the deferred envelope is a per-file/per-diff payload. Combining them would require deciding which file the payload references — out of scope. | Spec §H1 step 2. Future spec could relax this if session-mode deferred eval is needed. |
| `--print-prompt` interaction | `--print-prompt` still short-circuits before deferred-mode collection — it prints one rule's prompt and exits 0 (its existing semantics). If both `--emit-semantic-payload` and `--print-prompt` are passed, clap-rejects (mutually exclusive). | They're both adapter-internal debug surfaces; combining them is undefined and a hint of mis-use. |
| `--explain` interaction | Compatible. `--explain`'s per-rule outcome rows include `Deferred { reason: "subagent" }` for rules collected into the envelope. Stderr-only, doesn't affect the exit code or the JSON envelope. | Mirrors the `Dispatched` outcome for the direct-API semantic path; gives operators a way to see what would have been dispatched. |
| `DeferredVerdict` schema_version | Starts at `1`. Independent of `Verdict::SCHEMA_VERSION` (currently 2). Bumps independently. | Additive new shape — no reason to couple bumps. Telemetry's schema version is also independent; same pattern. |
| `hector_version` field | Stamped via `env!("CARGO_PKG_VERSION")` to match `Verdict` and `LogEntry::SessionInit`. | Existing convention. |
| `passed_checks` field on the envelope | List of rule IDs that ran-and-passed deterministically (i.e. script/ast rules that returned no violations during this check). Mirrors bully's payload exactly. | Spec §H1. The subagent uses this to know what was already covered; downstream `record-verdict` calls fill in the rest. |
| Telemetry on deferred path | Deterministic engines still emit their usual telemetry. **No** `SemanticVerdict` records are written by the runner in deferred mode — those are emitted later by the skill via `hector record-verdict` (H2). | Spec §H1. Avoids double-recording. |

---

## File structure

```
crates/hector-core/
├── src/
│   ├── lib.rs                          ← MODIFIED: pub mod verdict_deferred
│   ├── llm/
│   │   ├── mod.rs                      ← MODIFIED: claude-code-subagent arm in build_from_config
│   │   └── prompt.rs                   ← MODIFIED: build_evaluator_input(rules, primary, context)
│   ├── runner.rs                       ← MODIFIED: CheckOptions::emit_semantic_payload; CheckReport::deferred; deferred dispatch arms
│   └── verdict_deferred.rs             ← NEW: DeferredVerdict, DeferredPayload, DeferredRule, DEFERRED_SCHEMA_VERSION
└── tests/
    ├── llm_provider_subagent.rs        ← NEW: claude-code-subagent returns Ok(None), no stderr
    ├── deferred_verdict_shape.rs       ← NEW: serde shape locked via insta snapshot
    └── runner_deferred_mode.rs         ← NEW: runner collects rather than dispatches

crates/hector-cli/
├── src/
│   ├── cli.rs                          ← MODIFIED: emit_semantic_payload bool on Check
│   ├── main.rs                         ← MODIFIED: plumb the new arg
│   └── commands/
│       └── check.rs                    ← MODIFIED: emit DeferredVerdict JSON when present
└── tests/
    └── cli_e2e_emit_semantic_payload.rs   ← NEW: assert_cmd integration tests + insta snapshot

docs/
└── emit-semantic-payload.md            ← NEW: DeferredVerdict shape; adapter integration guide

CHANGELOG.md                            ← MODIFIED: Unreleased entry
plans/README.md                         ← MODIFIED (final task only): mark archived
```

The runner change keeps the dispatch arms readable: a new helper `should_defer(rule_engine: EngineKind, options: &CheckOptions) -> bool` replaces the existing `EngineKind::Semantic`/`EngineKind::Session` dispatch arms with a guard that, when defer is active, builds a `DeferredRule` and skips engine invocation. The function is one line; the arms stay flat.

---

## Risk / rollback

**Verdict-schema impact.** None. `Verdict` and `SCHEMA_VERSION` are untouched. `DeferredVerdict` is an additive sibling at its own schema version; consumers that expect `Verdict` and receive `DeferredVerdict` will see `"deferred": true` at the top level and can branch.

**Exit-code-contract impact.** None. `0` / `1` / `2` semantics on `hector check` are preserved: deterministic block still exits 2 (deferred path is suppressed); pass with no deferred work exits 0; pass with deferred work exits 0 (the spec is explicit: "deferred eval is not a block"). The new envelope rides on stdout exactly where a `Verdict` would.

**Telemetry-schema impact.** None. The runner emits the same `Check` / per-rule records for deterministic engines. Semantic + session telemetry shifts from the runner to the adapter (via H2's `record-verdict`); no `LogEntry` variant changes shape.

**Config-schema impact.** Additive: a new accepted value `claude-code-subagent` for `llm.provider`. Existing configs are unaffected. The trust fingerprint covers the change automatically because the LLM block is part of the canonicalized YAML.

**Performance.** Deferred mode is *faster* than direct dispatch — semantic rules cost an HTTP RTT to the LLM; collecting into a list is O(1) per rule. The deterministic path is unchanged.

**Rollback.** Pure addition. Removing the new module, the `CheckOptions` field, the CLI flag, and the provider arm restores the prior behaviour. No persisted state introduced.

**Coexistence with H2/H3.** H1 ships independently. H2 (`record-verdict`) is consumed by the adapter skill, not by the runner; the runner does not call it. H3 (adapter mode) reads the `DeferredVerdict` envelope and produces Claude Code's `hookSpecificOutput`; it depends on this plan landing but not vice versa.

---

## Phase 1 — Provider arm: `claude-code-subagent` returns `Ok(None)` without warning

Smallest piece, fully independent of the rest of the plan. The runner check that follows in Phase 4 reads the provider string out of the loaded config; this phase teaches `build_from_config` to *accept* the new value instead of rejecting it as `unknown LLM provider`.

### Task 1.1: Failing test — provider arm exists and is silent

**Files:**
- Create: `crates/hector-core/tests/llm_provider_subagent.rs`

- [ ] **Step 1.1.1: Write the failing test**

```rust
//! H1 — `provider: claude-code-subagent` is recognised by build_from_config.
//! Returns Ok(None) (so the runner knows to use the deferred path) and emits
//! NO stderr warning (distinct from the missing-API-key path, which warns).

use hector_core::config::LlmConfig;
use hector_core::llm::build_from_config;

#[test]
fn claude_code_subagent_provider_returns_none_without_warning() {
    let cfg = LlmConfig {
        provider: "claude-code-subagent".to_string(),
        model: "ignored".to_string(),
        api_key_env: None,
        base_url: None,
    };
    // The function returns `Result<Option<Box<dyn LlmClient>>>`. We assert it
    // is `Ok(None)` — no client is constructed, no error is raised.
    let result = build_from_config(&cfg).expect("subagent provider must not error");
    assert!(
        result.is_none(),
        "subagent provider must yield None — direct dispatch is disabled"
    );
}

#[test]
fn unknown_provider_still_errors() {
    // Regression: adding the subagent arm must not turn unknown providers
    // into silent passes.
    let cfg = LlmConfig {
        provider: "definitely-not-a-real-provider".to_string(),
        model: "ignored".to_string(),
        api_key_env: None,
        base_url: None,
    };
    let err = build_from_config(&cfg).expect_err("unknown provider must error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("claude-code-subagent"),
        "error message must list the new provider so users discover it: got {msg:?}"
    );
}
```

- [ ] **Step 1.1.2: Run test to verify it fails**

Run: `cargo test -p hector-core --test llm_provider_subagent`
Expected: FAIL — `unknown LLM provider 'claude-code-subagent'` from the existing `bail!` arm.

### Task 1.2: Implement the arm

**Files:**
- Modify: `crates/hector-core/src/llm/mod.rs` (the `match cfg.provider.as_str()` block in `build_from_config`; the closing `other =>` bail arm needs its supported-providers list updated)

- [ ] **Step 1.2.1: Add the arm**

Insert immediately before the `other =>` arm:

```rust
        "claude-code-subagent" => {
            // H1: the deferred-payload path. No LLM client is constructed;
            // the runner detects None and emits a DeferredVerdict that the
            // Claude Code adapter routes to an in-session subagent. We do
            // NOT emit the missing-API-key warning here — that path is for
            // misconfiguration; this is an opt-in routing choice.
            Ok(None)
        }
```

Update the `other =>` bail to list the new provider:

```rust
        other => {
            bail!("unknown LLM provider `{other}`. Supported: anthropic, claude-code-subagent, ollama, openrouter")
        }
```

- [ ] **Step 1.2.2: Run test to verify it passes**

Run: `cargo test -p hector-core --test llm_provider_subagent`
Expected: PASS, both tests.

- [ ] **Step 1.2.3: Run workspace tests to confirm no regression**

Run: `cargo test --workspace`
Expected: previous totals, all green. The existing provider tests (`tests/anthropic.rs`, `tests/openai_compat.rs`) do not touch the new arm and stay unchanged.

- [ ] **Step 1.2.4: Commit**

```bash
git add crates/hector-core/src/llm/mod.rs crates/hector-core/tests/llm_provider_subagent.rs
git commit -m "feat(llm): recognise provider: claude-code-subagent (H1 phase 1)

Add a new arm to build_from_config that yields Ok(None) — no LLM
client, no stderr warning — for the subscription-mediated semantic
eval path. The runner consults this to choose between dispatching
and emitting a deferred-evaluation payload."
```

---

## Phase 2 — Prompt helper: `build_evaluator_input`

The deferred payload's `_evaluator_input` field is a single string the subagent reads to know what rules to evaluate and which file/diff to evaluate them against. Bully assembles this as a system + user concatenation; Hector already has `build_prompt_split` producing the same `(system, user)` tuple for Anthropic's split-role API. This phase adds a thin wrapper that concatenates the tuple into one evaluator-input string — same content the model would have received in the direct-API path, written into the payload instead.

### Task 2.1: Failing test — byte-identical to `build_prompt_split` concatenated

**Files:**
- Modify: `crates/hector-core/src/llm/prompt.rs` (extend the existing `#[cfg(test)] mod tests` block at the bottom of the file)

- [ ] **Step 2.1.1: Add the failing test**

Append to the existing `mod tests`:

```rust
    #[test]
    fn build_evaluator_input_concatenates_split_prompt() {
        // H1: `_evaluator_input` is the byte-identical concatenation of the
        // (system, user) tuple `build_prompt_split` already produces. Locking
        // this assertion means the subagent and the direct-API path read
        // exactly the same content — no prompt drift between routes.
        let rule = test_rule("no-debug", "no DEBUG prints in committed code");
        let rules = vec![("no-debug", &rule)];
        let (sys, usr) = build_prompt_split(&rules, "let x = 1;\n", None);
        let evaluator = build_evaluator_input(&rules, "let x = 1;\n", None);
        assert_eq!(
            evaluator,
            format!("{sys}\n{usr}"),
            "evaluator input must be (system, user) joined with one newline"
        );
    }

    #[test]
    fn build_evaluator_input_with_context() {
        let rule = test_rule("no-foo", "describe foo");
        let rules = vec![("no-foo", &rule)];
        let (sys, usr) = build_prompt_split(&rules, "primary content", Some("ctx content"));
        let evaluator = build_evaluator_input(&rules, "primary content", Some("ctx content"));
        assert_eq!(evaluator, format!("{sys}\n{usr}"));
        // Sanity: both ends are present in the evaluator string.
        assert!(evaluator.contains("<TRUSTED_POLICY>"));
        assert!(evaluator.contains("<UNTRUSTED_EVIDENCE>"));
        assert!(evaluator.contains("ctx content"));
    }
```

- [ ] **Step 2.1.2: Run tests to verify they fail**

Run: `cargo test -p hector-core --lib llm::prompt::tests::build_evaluator_input`
Expected: FAIL — `cannot find function build_evaluator_input in this scope`.

### Task 2.2: Implement `build_evaluator_input`

**Files:**
- Modify: `crates/hector-core/src/llm/prompt.rs` (add a new `pub fn` next to `build_prompt_split`)

- [ ] **Step 2.2.1: Add the function**

Insert immediately after `build_prompt_split`'s closing brace:

```rust
/// Render the bully-compatible `_evaluator_input` string for H1's deferred
/// payload. Concatenates the `(system, user)` tuple from
/// [`build_prompt_split`] with a single newline — byte-identical to what
/// the model would receive on the direct-API path, with the same
/// sentinel-tag boundary and the same content sanitization. The subagent
/// reads this verbatim.
pub fn build_evaluator_input(
    rules: &[(&str, &Rule)],
    primary: &str,
    context: Option<&str>,
) -> String {
    let (system, user) = build_prompt_split(rules, primary, context);
    format!("{system}\n{user}")
}
```

- [ ] **Step 2.2.2: Run tests to verify they pass**

Run: `cargo test -p hector-core --lib llm::prompt::tests::build_evaluator_input`
Expected: PASS, both tests.

- [ ] **Step 2.2.3: Commit**

```bash
git add crates/hector-core/src/llm/prompt.rs
git commit -m "feat(llm/prompt): add build_evaluator_input wrapper (H1 phase 2)

The subagent path needs a single-string concatenation of the
(system, user) tuple build_prompt_split produces. Wrap that
concatenation in a named helper so the call sites (runner deferred
arm, future tests) read intent-first."
```

---

## Phase 3 — `DeferredVerdict` types and serde shape

This phase defines the JSON envelope the runner and CLI emit when deferred work is collected. Lives in a new module so `verdict.rs` stays focused on the locked `Verdict` shape. Includes an insta snapshot lock on the JSON shape so future contributors can't drift the wire format without intentional review.

### Task 3.1: Failing test — empty `DeferredVerdict` serializes to the documented shape

**Files:**
- Create: `crates/hector-core/tests/deferred_verdict_shape.rs`

- [ ] **Step 3.1.1: Write the failing test**

```rust
//! H1 — DeferredVerdict serde-shape lockfile. The wire format is part of
//! the adapter contract (`adapters/claude-code/hooks/hook.sh` consumes it
//! via `jq`); changing it without bumping DEFERRED_SCHEMA_VERSION is a
//! silent break.

use hector_core::verdict_deferred::{
    DeferredPayload, DeferredRule, DeferredVerdict, DEFERRED_SCHEMA_VERSION,
};

#[test]
fn deferred_schema_version_is_one() {
    assert_eq!(DEFERRED_SCHEMA_VERSION, 1);
}

#[test]
fn empty_deferred_verdict_serializes_to_canonical_shape() {
    let v = DeferredVerdict {
        schema_version: DEFERRED_SCHEMA_VERSION,
        deferred: true,
        hector_version: "0.2.x".to_string(),
        passed_checks: vec![],
        payload: DeferredPayload {
            file: "src/foo.rs".into(),
            diff: String::new(),
            passed_checks: vec![],
            evaluate: vec![],
            evaluator_input: String::new(),
        },
        elapsed_ms: 0,
    };
    let json = serde_json::to_value(&v).unwrap();
    insta::assert_json_snapshot!(json, @r###"
    {
      "schema_version": 1,
      "deferred": true,
      "hector_version": "0.2.x",
      "passed_checks": [],
      "payload": {
        "file": "src/foo.rs",
        "diff": "",
        "passed_checks": [],
        "evaluate": [],
        "_evaluator_input": ""
      },
      "elapsed_ms": 0
    }
    "###);
}

#[test]
fn deferred_verdict_with_two_rules_serializes() {
    let v = DeferredVerdict {
        schema_version: DEFERRED_SCHEMA_VERSION,
        deferred: true,
        hector_version: "0.2.x".to_string(),
        passed_checks: vec!["det-1".into(), "det-2".into()],
        payload: DeferredPayload {
            file: "src/foo.rs".into(),
            diff: "@@ -1,1 +1,1 @@\n-old\n+new\n".into(),
            passed_checks: vec!["det-1".into(), "det-2".into()],
            evaluate: vec![
                DeferredRule {
                    id: "no-debug".into(),
                    description: "no DEBUG prints".into(),
                    severity: "error".into(),
                    engine: "semantic".into(),
                },
                DeferredRule {
                    id: "schema-needs-migration".into(),
                    description: "schema edits require a migration file".into(),
                    severity: "warning".into(),
                    engine: "session".into(),
                },
            ],
            evaluator_input: "<TRUSTED_POLICY>...</UNTRUSTED_EVIDENCE>".into(),
        },
        elapsed_ms: 42,
    };
    insta::assert_json_snapshot!(serde_json::to_value(&v).unwrap());
}
```

- [ ] **Step 3.1.2: Run test to verify it fails**

Run: `cargo test -p hector-core --test deferred_verdict_shape`
Expected: FAIL — `unresolved import hector_core::verdict_deferred`.

### Task 3.2: Create the module

**Files:**
- Create: `crates/hector-core/src/verdict_deferred.rs`
- Modify: `crates/hector-core/src/lib.rs` (add `pub mod verdict_deferred;`)

- [ ] **Step 3.2.1: Add the module declaration**

In `crates/hector-core/src/lib.rs`, alongside the existing `pub mod verdict;`:

```rust
pub mod verdict_deferred;
```

- [ ] **Step 3.2.2: Implement the types**

Create `crates/hector-core/src/verdict_deferred.rs`:

```rust
//! H1: deferred-evaluation envelope for the Claude Code subagent path.
//!
//! When `llm.provider: claude-code-subagent` (or `--emit-semantic-payload`)
//! is active and at least one `engine: semantic` or `engine: session` rule
//! survives scope/skip/diff-prefilter, the runner emits this envelope
//! instead of dispatching the rules. The Claude Code adapter's hook script
//! wraps `payload` in `hookSpecificOutput.additionalContext`; the
//! interpreter skill dispatches an in-session subagent against
//! `_evaluator_input`.
//!
//! Wire-format stability: changes to the shape MUST bump
//! [`DEFERRED_SCHEMA_VERSION`]. The shape is locked by an insta snapshot
//! in `tests/deferred_verdict_shape.rs`.

use serde::{Deserialize, Serialize};

/// Schema version for the deferred-evaluation envelope. Independent of
/// [`crate::verdict::SCHEMA_VERSION`] — the two schemas evolve separately.
pub const DEFERRED_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredVerdict {
    pub schema_version: u32,
    /// Always `true` for this envelope. The redundancy is intentional:
    /// downstream consumers can branch on a single top-level boolean
    /// without parsing `schema_version` or type discriminating against
    /// `Verdict`'s `status: pass | warn | block`.
    pub deferred: bool,
    pub hector_version: String,
    /// Rule IDs that ran-and-passed deterministically during this check.
    /// Mirrors `Verdict::passed_checks`; the subagent uses this to know
    /// what was already covered.
    pub passed_checks: Vec<String>,
    pub payload: DeferredPayload,
    pub elapsed_ms: u64,
}

/// The bully-shaped payload the Claude Code skill consumes. Field names
/// match bully byte-for-byte so the ported skill text needs no rewriting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredPayload {
    pub file: String,
    /// Unified diff (empty string for whole-file checks). The subagent
    /// uses this to identify changed regions when evaluating session
    /// rules; semantic rules see it as additional context.
    pub diff: String,
    pub passed_checks: Vec<String>,
    pub evaluate: Vec<DeferredRule>,
    /// The full evaluator prompt — system + user from
    /// [`crate::llm::prompt::build_evaluator_input`] — rendered as a
    /// single string. The skill passes this verbatim to the subagent.
    /// Field name uses an underscore prefix to match bully's wire format.
    #[serde(rename = "_evaluator_input")]
    pub evaluator_input: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredRule {
    pub id: String,
    pub description: String,
    /// `"error"` | `"warning"`. Stringly-typed in the wire format to
    /// match bully; converted from [`crate::config::Severity`] at
    /// payload-construction time.
    pub severity: String,
    /// `"semantic"` | `"session"`. Per spec §6 Q1, session rules are
    /// deferred under the same flag and identified here. The skill
    /// routes all entries through the same subagent regardless.
    pub engine: String,
}
```

- [ ] **Step 3.2.3: Run test to verify the snapshot matches**

Run: `cargo test -p hector-core --test deferred_verdict_shape`
Expected: First run, insta will create the snapshot files. Accept them with `cargo insta accept` (or hand-verify the first snapshot output matches the inline `@r###"…"###` block). Subsequent runs PASS.

- [ ] **Step 3.2.4: Commit**

```bash
git add crates/hector-core/src/lib.rs crates/hector-core/src/verdict_deferred.rs crates/hector-core/tests/deferred_verdict_shape.rs
git commit -m "feat(verdict_deferred): DeferredVerdict envelope (H1 phase 3)

Defines the JSON shape the runner emits when `engine: semantic` or
`engine: session` rules are deferred to an in-session Claude Code
subagent. Wire format matches bully byte-for-byte; shape is locked
by an insta snapshot in deferred_verdict_shape.rs.

DEFERRED_SCHEMA_VERSION is independent of Verdict::SCHEMA_VERSION."
```

---

## Phase 4 — Runner: collect deferred rules instead of dispatching

This phase teaches `HectorEngine` to short-circuit `Semantic` and `Session` dispatches when deferred mode is active, returning the would-have-been-evaluated rules in a new `CheckReport::deferred` field that the CLI inspects.

### Task 4.1: Failing test — deferred mode collects rules

**Files:**
- Create: `crates/hector-core/tests/runner_deferred_mode.rs`

- [ ] **Step 4.1.1: Write the failing test**

```rust
//! H1 — runner-level test that `emit_semantic_payload: true` causes
//! `Semantic` and `Session` rules to be collected into the deferred
//! envelope rather than dispatched.

use hector_core::runner::{CheckInput, CheckOptions, HectorEngine};
use std::collections::HashSet;
use std::fs;
use tempfile::tempdir;

const CONFIG_YAML: &str = r#"
schema_version: 2
trust:
  fingerprint: PLACEHOLDER
llm:
  provider: claude-code-subagent
  model: ignored
rules:
  no-debug:
    description: no DEBUG prints in committed code
    engine: semantic
    scope: ["**/*.rs"]
    severity: error
"#;

fn write_trusted_config(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    fs::write(&path, CONFIG_YAML).unwrap();
    // Compute and rewrite the trust fingerprint so HectorEngine::load accepts it.
    let yaml = fs::read_to_string(&path).unwrap();
    let new = hector_core::trust::write_trust_block(&yaml).unwrap();
    fs::write(&path, new).unwrap();
    path
}

#[test]
fn deferred_mode_collects_semantic_rule() {
    let tmp = tempdir().unwrap();
    let config = write_trusted_config(tmp.path());
    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() { println!(\"DEBUG\"); }\n").unwrap();

    let opts = CheckOptions {
        rules: HashSet::new(),
        explain: false,
        emit_semantic_payload: true,
    };
    let engine = HectorEngine::builder()
        .with_options(opts)
        .load(&config)
        .expect("config loads with subagent provider");
    let content = fs::read_to_string(&src).unwrap();
    let report = engine
        .check_with_explain(CheckInput::File { path: src, content })
        .expect("check succeeds");

    let deferred = report
        .deferred
        .as_ref()
        .expect("deferred envelope must be present when a semantic rule is in scope");
    assert_eq!(deferred.payload.evaluate.len(), 1);
    assert_eq!(deferred.payload.evaluate[0].id, "no-debug");
    assert_eq!(deferred.payload.evaluate[0].engine, "semantic");
    // The deterministic verdict carries no semantic violations.
    assert!(
        report
            .verdict
            .violations
            .iter()
            .all(|v| v.engine != hector_core::verdict::Engine::Semantic),
        "deferred semantic rules must not produce verdict violations"
    );
}
```

- [ ] **Step 4.1.2: Run test to verify it fails**

Run: `cargo test -p hector-core --test runner_deferred_mode`
Expected: FAIL — `no field emit_semantic_payload on type CheckOptions` and `no field deferred on type CheckReport`.

### Task 4.2: Add `CheckOptions::emit_semantic_payload` and `CheckReport::deferred`

**Files:**
- Modify: `crates/hector-core/src/runner.rs` (the `CheckOptions` struct around line 28, the `CheckReport` struct around line 61)

- [ ] **Step 4.2.1: Extend `CheckOptions`**

Locate the struct (`grep -n 'pub struct CheckOptions' crates/hector-core/src/runner.rs`) and add the field with serde defaults so older library callers continue compiling:

```rust
pub struct CheckOptions {
    pub rules: HashSet<String>,
    pub explain: bool,
    /// H1: when true, `engine: semantic` and `engine: session` rules are
    /// not dispatched — they are collected into [`CheckReport::deferred`]
    /// for an in-session Claude Code subagent to evaluate.
    pub emit_semantic_payload: bool,
}
```

`CheckOptions` already derives `Default` (verify by grepping the file); the new field defaults to `false`, preserving existing call-sites.

- [ ] **Step 4.2.2: Extend `CheckReport`**

```rust
pub struct CheckReport {
    pub verdict: Verdict,
    pub explain: Vec<RuleExplain>,
    /// H1: present when `CheckOptions::emit_semantic_payload` was true
    /// and at least one semantic/session rule survived scope/skip/
    /// diff-prefilter. `None` otherwise. The CLI inspects this to
    /// decide whether to emit a `DeferredVerdict` or a standard
    /// `Verdict`.
    pub deferred: Option<crate::verdict_deferred::DeferredVerdict>,
}
```

Update every internal `CheckReport { … }` constructor in the file to include `deferred: None`. There are 2-3 in `runner.rs`; find them with `grep -n 'CheckReport {' crates/hector-core/src/runner.rs`.

### Task 4.3: Implement the deferred dispatch

**Files:**
- Modify: `crates/hector-core/src/runner.rs` (the two dispatch arms in `dispatch_one_rule` that handle `EngineKind::Semantic` and `EngineKind::Session`, around lines 568-580 per the earlier grep)

- [ ] **Step 4.3.1: Add a `should_defer` helper**

Add near the top of `runner.rs` (alongside the existing `engine_kind_to_verdict_engine` helper):

```rust
/// H1: decide whether a semantic or session rule should be collected
/// into the deferred envelope instead of dispatched. Returns true only
/// when the option is set AND the engine is one of the two LLM-dispatch
/// engines — `Script` and `Ast` always run.
fn should_defer(engine: EngineKind, options: &CheckOptions) -> bool {
    options.emit_semantic_payload
        && matches!(engine, EngineKind::Semantic | EngineKind::Session)
}
```

- [ ] **Step 4.3.2: Plumb a `Vec<DeferredRule>` through the check pipeline**

Locate `check_with_explain` (`grep -n 'pub fn check_with_explain' crates/hector-core/src/runner.rs`). Before the rule-iteration loop, declare:

```rust
let mut deferred_rules: Vec<crate::verdict_deferred::DeferredRule> = Vec::new();
let mut deferred_file: Option<String> = None;
let mut deferred_diff: String = String::new();
```

Inside the per-rule body, immediately before the `EngineKind::Semantic` or `EngineKind::Session` dispatch, branch:

```rust
if should_defer(rule.engine, &self.options) {
    deferred_file.get_or_insert_with(|| input_file_string(&input));
    if deferred_diff.is_empty() {
        deferred_diff = input_diff_string(&input);
    }
    deferred_rules.push(crate::verdict_deferred::DeferredRule {
        id: rule_id.clone(),
        description: rule.description.clone(),
        severity: severity_string(rule.severity),
        engine: match rule.engine {
            EngineKind::Semantic => "semantic".into(),
            EngineKind::Session => "session".into(),
            _ => unreachable!("should_defer guards on Semantic/Session"),
        },
    });
    // Emit a Deferred explain row so --explain users see what was deferred.
    if self.options.explain {
        explain.push(RuleExplain {
            rule_id: rule_id.clone(),
            engine: rule.engine,
            outcome: crate::runner::ExplainOutcome::Skipped {
                reason: "deferred_subagent".into(),
            },
        });
    }
    continue; // skip dispatch
}
```

Add the helper functions at the bottom of `runner.rs`:

```rust
fn input_file_string(input: &CheckInput) -> String {
    match input {
        CheckInput::File { path, .. } => path.display().to_string(),
        CheckInput::Diff { file, .. } => file.display().to_string(),
    }
}

fn input_diff_string(input: &CheckInput) -> String {
    match input {
        CheckInput::File { .. } => String::new(),
        CheckInput::Diff { unified_diff, .. } => unified_diff.clone(),
    }
}

fn severity_string(s: crate::config::Severity) -> String {
    match s {
        crate::config::Severity::Error => "error".into(),
        crate::config::Severity::Warning => "warning".into(),
    }
}
```

After the rule-iteration loop, before constructing `CheckReport`, build the envelope:

```rust
let deferred = if deferred_rules.is_empty() {
    None
} else {
    // Build the (rules, primary, context) tuple the same way the semantic
    // engine would have, but pass it to `build_evaluator_input` instead of
    // an LLM client. We only need the rule slice in the format the prompt
    // builder expects.
    let rule_refs: Vec<(&str, &crate::config::Rule)> = deferred_rules
        .iter()
        .filter_map(|d| {
            self.config_rule(&d.id).map(|r| (d.id.as_str(), r))
        })
        .collect();
    let primary = match &input {
        CheckInput::File { content, .. } => content.clone(),
        CheckInput::Diff { unified_diff, .. } => unified_diff.clone(),
    };
    let evaluator_input =
        crate::llm::prompt::build_evaluator_input(&rule_refs, &primary, None);

    Some(crate::verdict_deferred::DeferredVerdict {
        schema_version: crate::verdict_deferred::DEFERRED_SCHEMA_VERSION,
        deferred: true,
        hector_version: env!("CARGO_PKG_VERSION").to_string(),
        passed_checks: verdict.passed_checks.clone(),
        payload: crate::verdict_deferred::DeferredPayload {
            file: deferred_file.unwrap_or_default(),
            diff: deferred_diff,
            passed_checks: verdict.passed_checks.clone(),
            evaluate: deferred_rules,
            evaluator_input,
        },
        elapsed_ms: verdict.elapsed_ms,
    })
};
```

Update the final return to `CheckReport { verdict, explain, deferred }`.

You will need a `config_rule(&str) -> Option<&Rule>` accessor on `HectorEngine`. If one doesn't exist, add it:

```rust
impl HectorEngine {
    /// Lookup a rule by id from the loaded config. Used by H1 to
    /// resolve `DeferredRule` ids back to their full definitions when
    /// building the evaluator-input string.
    pub fn config_rule(&self, id: &str) -> Option<&crate::config::Rule> {
        self.config.rules.get(id)
    }
}
```

(Adjust the field path if `config.rules` is structured differently — verify with `grep -n 'pub struct Config' crates/hector-core/src/config/types.rs`.)

- [ ] **Step 4.3.3: Run the failing test to verify it now passes**

Run: `cargo test -p hector-core --test runner_deferred_mode`
Expected: PASS.

- [ ] **Step 4.3.4: Run the workspace tests to confirm no regression**

Run: `cargo test --workspace`
Expected: previous totals, all green. The new `deferred: None` default keeps every other `CheckReport` construction site behaviorally identical.

- [ ] **Step 4.3.5: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4.3.6: Commit**

```bash
git add crates/hector-core/src/runner.rs crates/hector-core/tests/runner_deferred_mode.rs
git commit -m "feat(runner): collect semantic/session rules when deferred (H1 phase 4)

New CheckOptions::emit_semantic_payload toggles the deferred path:
Semantic and Session rules are collected into a DeferredVerdict
envelope on CheckReport::deferred instead of being dispatched.
Deterministic engines (script, ast) and the rest of the pipeline
(baseline, telemetry of deterministic outcomes) are unchanged.

The --explain row for a deferred rule reports
Skipped { reason: deferred_subagent } so operators can see what
would have been dispatched."
```

---

## Phase 5 — CLI: `--emit-semantic-payload` flag and `DeferredVerdict` output

This phase wires the runner change to the user-facing CLI: a new flag, plumbed through `commands::check::run`, that emits the envelope on stdout when present and uses the correct exit code.

### Task 5.1: Failing test — flag is rejected today

**Files:**
- Create: `crates/hector-cli/tests/cli_e2e_emit_semantic_payload.rs`

- [ ] **Step 5.1.1: Write the failing test**

```rust
//! H1 — end-to-end coverage that `hector check --emit-semantic-payload`
//! produces the expected envelope on stdout.

use assert_cmd::Command;
use predicates::str::contains;
use std::fs;
use tempfile::tempdir;

const CONFIG_YAML: &str = r#"
schema_version: 2
trust:
  fingerprint: PLACEHOLDER
llm:
  provider: claude-code-subagent
  model: ignored
rules:
  no-debug:
    description: no DEBUG prints in committed code
    engine: semantic
    scope: ["**/*.rs"]
    severity: error
"#;

fn write_trusted_config(dir: &std::path::Path) {
    let path = dir.join(".hector.yml");
    fs::write(&path, CONFIG_YAML).unwrap();
    let yaml = fs::read_to_string(&path).unwrap();
    let new = hector_core::trust::write_trust_block(&yaml).unwrap();
    fs::write(&path, new).unwrap();
}

#[test]
fn flag_emits_deferred_verdict_envelope() {
    let tmp = tempdir().unwrap();
    write_trusted_config(tmp.path());
    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() {}\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .arg("check")
        .arg("--config").arg(tmp.path().join(".hector.yml"))
        .arg("--file").arg(&src)
        .arg("--emit-semantic-payload")
        .arg("--format").arg("json")
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(out).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .expect(&format!("stdout must be valid JSON, got: {stdout}"));
    assert_eq!(v["deferred"], serde_json::Value::Bool(true));
    assert_eq!(v["schema_version"], serde_json::Value::Number(1.into()));
    assert_eq!(
        v["payload"]["evaluate"][0]["id"].as_str(),
        Some("no-debug")
    );
    assert!(v["payload"]["_evaluator_input"].as_str().unwrap().contains("no-debug"));
}

#[test]
fn flag_rejects_combined_with_session() {
    let tmp = tempdir().unwrap();
    write_trusted_config(tmp.path());
    Command::cargo_bin("hector")
        .unwrap()
        .arg("check")
        .arg("--config").arg(tmp.path().join(".hector.yml"))
        .arg("--session")
        .arg("--emit-semantic-payload")
        .assert()
        .failure()
        .stderr(contains("cannot be used with"));
}

#[test]
fn flag_omitted_means_no_envelope() {
    // Sanity: without the flag, the CLI emits the standard Verdict shape,
    // not the DeferredVerdict envelope. Asserts the additive nature of the
    // change — no behaviour drift for existing call-sites.
    let tmp = tempdir().unwrap();
    // Use a non-subagent provider so direct-dispatch is attempted but the
    // missing API key makes semantic skip silently.
    let cfg = CONFIG_YAML.replace("claude-code-subagent", "anthropic");
    let path = tmp.path().join(".hector.yml");
    fs::write(&path, cfg).unwrap();
    let yaml = fs::read_to_string(&path).unwrap();
    let new = hector_core::trust::write_trust_block(&yaml).unwrap();
    fs::write(&path, new).unwrap();
    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() {}\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .arg("check")
        .arg("--config").arg(&path)
        .arg("--file").arg(&src)
        .arg("--format").arg("json")
        .assert()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(v.get("deferred").is_none(), "no flag, no envelope");
    assert!(v.get("status").is_some(), "standard Verdict has status field");
}
```

- [ ] **Step 5.1.2: Run test to verify it fails**

Run: `cargo test -p hector-cli --test cli_e2e_emit_semantic_payload`
Expected: FAIL — clap rejects the unknown `--emit-semantic-payload` flag.

### Task 5.2: Add the clap flag

**Files:**
- Modify: `crates/hector-cli/src/cli.rs` (the `Command::Check { … }` variant)

- [ ] **Step 5.2.1: Add the flag and mutual-exclusion**

Inside the `Check` variant, alongside the existing `print_prompt` arg, add:

```rust
        /// H1: instead of dispatching `engine: semantic` and
        /// `engine: session` rules to the configured LLM, collect them
        /// into a `DeferredVerdict` JSON envelope for an in-session
        /// Claude Code subagent to evaluate. Adapter-internal.
        #[arg(
            long = "emit-semantic-payload",
            conflicts_with = "session",
            conflicts_with = "print_prompt"
        )]
        emit_semantic_payload: bool,
```

- [ ] **Step 5.2.2: Plumb through main and check::run**

Modify `crates/hector-cli/src/main.rs` to pass the new arg into `commands::check::run`. Add the parameter to `check::run`'s signature and to its `CheckOptions` construction.

In `crates/hector-cli/src/commands/check.rs`:

```rust
#[allow(clippy::too_many_arguments)]
pub fn run(
    file: Option<PathBuf>,
    diff: Option<PathBuf>,
    session: bool,
    format: OutputFormat,
    config: &Path,
    rules: Vec<String>,
    explain: bool,
    print_prompt: bool,
    emit_semantic_payload: bool,   // NEW
) -> Result<i32> {
```

In the `CheckOptions { … }` construction near the top of the function:

```rust
    let options = CheckOptions {
        rules: rule_set,
        explain,
        emit_semantic_payload,
    };
```

### Task 5.3: Branch on `report.deferred` in the emit path

**Files:**
- Modify: `crates/hector-cli/src/commands/check.rs` (the `(Some(f), None)` and `(None, Some(d))` arms of the input match)

- [ ] **Step 5.3.1: Add the deferred-emit branch**

In the file-input arm, after computing `report`, before the existing `emit(&report.verdict, format)?` call, branch:

```rust
            if let Some(d) = &report.deferred {
                // Deterministic block still wins — never emit deferred on
                // top of an error-severity violation. (Verdict::status was
                // built from the deterministic violations only; semantic/
                // session were collected, not dispatched.)
                if matches!(report.verdict.status, Status::Block) {
                    emit(&report.verdict, format)?;
                    return Ok(2);
                }
                emit_deferred(d, format)?;
                return Ok(0);
            }
```

Add the new helper at the bottom of the file:

```rust
fn emit_deferred(d: &hector_core::verdict_deferred::DeferredVerdict, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json | OutputFormat::Human => {
            // For deferred envelopes, JSON is always the wire format —
            // the adapter parses it via `jq`. `--format human` falls back
            // to JSON because the envelope is machine-only.
            println!("{}", serde_json::to_string_pretty(d)?);
        }
    }
    Ok(())
}
```

Apply the same branch in the diff-input arm (do not re-run for every changed file — instead, after the aggregation loop, check whether *any* per-file report carried a `deferred` envelope and merge them; for the first cut, document the limitation and defer the multi-file aggregation to a follow-up).

For the diff path, the simplest first cut is to reject `--emit-semantic-payload` combined with `--diff`:

```rust
        (None, Some(_)) if emit_semantic_payload => {
            eprintln!("ERROR: --emit-semantic-payload is not supported with --diff yet (multi-file envelope aggregation is a follow-up)");
            return Ok(1);
        }
```

Add this guard *before* the existing diff arm. Update the H1 plan's "follow-up" section in the spec/changelog accordingly.

- [ ] **Step 5.3.2: Run the integration test**

Run: `cargo test -p hector-cli --test cli_e2e_emit_semantic_payload`
Expected: PASS, all three tests.

- [ ] **Step 5.3.3: Run the full workspace**

Run: `cargo test --workspace`
Expected: previous totals, all green.

- [ ] **Step 5.3.4: Run clippy + fmt**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: clean.

- [ ] **Step 5.3.5: Commit**

```bash
git add crates/hector-cli/src/cli.rs crates/hector-cli/src/main.rs crates/hector-cli/src/commands/check.rs crates/hector-cli/tests/cli_e2e_emit_semantic_payload.rs
git commit -m "feat(cli): hector check --emit-semantic-payload (H1 phase 5)

When set, semantic and session rules are collected into a
DeferredVerdict envelope emitted on stdout (JSON) instead of being
dispatched to the LLM. Exit code stays at the locked contract:
deterministic block → 2 (deferred suppressed), no block + envelope
→ 0, no block + no envelope → 0.

Mutually exclusive with --session and --print-prompt. --diff
combined with the flag is rejected for now; multi-file envelope
aggregation is a follow-up."
```

---

## Phase 6 — Block-deterministic-suppresses-deferred regression test

The spec is explicit: when a deterministic rule blocks, the deferred path is suppressed entirely (exit 2, no payload). This is encoded in Phase 5's `if matches!(status, Block)` branch — add a dedicated test so the behaviour can't drift.

### Task 6.1: Test — script-rule block with a surviving semantic rule still exits 2

**Files:**
- Modify: `crates/hector-cli/tests/cli_e2e_emit_semantic_payload.rs` (append a new test)

- [ ] **Step 6.1.1: Write the test**

Append to the existing file:

```rust
#[test]
fn deterministic_block_suppresses_deferred_envelope() {
    // A script rule that exits non-zero (block) plus a semantic rule
    // that would be deferred. The expected behaviour: the script
    // violation is the verdict, exit 2; no DeferredVerdict on stdout.
    let tmp = tempdir().unwrap();
    let cfg = r#"
schema_version: 2
trust:
  fingerprint: PLACEHOLDER
llm:
  provider: claude-code-subagent
  model: ignored
rules:
  no-debug-script:
    description: no DEBUG via grep
    engine: script
    scope: ["**/*.rs"]
    severity: error
    script: "grep -n 'DEBUG' {file} && exit 1 || exit 0"
    capabilities:
      network: false
      writes: none
  no-debug-semantic:
    description: no DEBUG prints in committed code
    engine: semantic
    scope: ["**/*.rs"]
    severity: error
"#;
    let path = tmp.path().join(".hector.yml");
    fs::write(&path, cfg).unwrap();
    let yaml = fs::read_to_string(&path).unwrap();
    let new = hector_core::trust::write_trust_block(&yaml).unwrap();
    fs::write(&path, new).unwrap();
    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() { println!(\"DEBUG\"); }\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .arg("check")
        .arg("--config").arg(&path)
        .arg("--file").arg(&src)
        .arg("--emit-semantic-payload")
        .arg("--format").arg("json")
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(v.get("deferred").is_none(), "block suppresses deferred envelope");
    assert_eq!(v["status"].as_str(), Some("block"));
}
```

- [ ] **Step 6.1.2: Run and confirm**

Run: `cargo test -p hector-cli --test cli_e2e_emit_semantic_payload deterministic_block_suppresses_deferred_envelope`
Expected: PASS.

- [ ] **Step 6.1.3: Commit**

```bash
git add crates/hector-cli/tests/cli_e2e_emit_semantic_payload.rs
git commit -m "test(cli): block-deterministic suppresses deferred envelope (H1 phase 6)"
```

---

## Phase 7 — Docs + CHANGELOG + plan archive

### Task 7.1: Write `docs/emit-semantic-payload.md`

**Files:**
- Create: `docs/emit-semantic-payload.md`

- [ ] **Step 7.1.1: Document the envelope and adapter integration**

```markdown
# `hector check --emit-semantic-payload`

Adapter-internal flag for the Claude Code subagent path. When set, semantic
and session rules are collected into a `DeferredVerdict` JSON envelope
instead of being dispatched to the configured LLM.

Activated by either:
- `llm.provider: claude-code-subagent` in `.hector.yml` (end-user-facing),
- or the long-only `--emit-semantic-payload` CLI flag (adapter-internal,
  used for explicit invocations and tests).

## Envelope shape

`schema_version: 1`. Independent of `Verdict::SCHEMA_VERSION`.

\`\`\`json
{
  "schema_version": 1,
  "deferred": true,
  "hector_version": "0.2.x",
  "passed_checks": ["det-rule-1", "det-rule-2"],
  "payload": {
    "file": "src/foo.rs",
    "diff": "@@ -1,1 +1,1 @@\n-old\n+new\n",
    "passed_checks": ["det-rule-1", "det-rule-2"],
    "evaluate": [
      {
        "id": "no-debug",
        "description": "no DEBUG prints in committed code",
        "severity": "error",
        "engine": "semantic"
      }
    ],
    "_evaluator_input": "<TRUSTED_POLICY>…</UNTRUSTED_EVIDENCE>"
  },
  "elapsed_ms": 42
}
\`\`\`

## Exit-code semantics

| Outcome | Exit code | Stdout |
|---|---|---|
| Deterministic block | `2` | Standard `Verdict` |
| Pass + deferred non-empty | `0` | `DeferredVerdict` envelope |
| Pass + no deferred | `0` | Standard `Verdict` |

Deferred eval is not a block — the verdict is decided later by the
in-session subagent.

## Limitations (0.2.x)

- `--diff` combined with `--emit-semantic-payload` is rejected; multi-file
  envelope aggregation is a follow-up.
- The envelope assumes a single primary file. `engine: session` rules
  that span multiple changed files still produce one envelope; the
  subagent receives every session-rule definition but only the primary
  file/diff.
```

- [ ] **Step 7.1.2: Commit**

```bash
git add docs/emit-semantic-payload.md
git commit -m "docs(h1): document --emit-semantic-payload envelope and exit codes"
```

### Task 7.2: CHANGELOG entry

**Files:**
- Modify: `CHANGELOG.md` (insert under `## Unreleased`)

- [ ] **Step 7.2.1: Add the entry**

Above the existing E2 / OpenCode / macOS-warn entries (newest first), insert:

```markdown
### Subagent semantic-eval — deferred-payload path (H1)

- New CLI flag `hector check --emit-semantic-payload` and new config value `llm.provider: claude-code-subagent`. When either is active, `engine: semantic` and `engine: session` rules are collected into a `DeferredVerdict` JSON envelope on stdout instead of being dispatched to the configured LLM. The envelope is byte-compatible with bully's `additionalContext` payload — the Claude Code adapter (H3, separate plan) wraps it for in-session subagent dispatch.
- Exit code semantics unchanged: deterministic block → 2 (deferred suppressed); pass + envelope → 0; pass + no envelope → 0.
- New module `hector_core::verdict_deferred` exposes `DeferredVerdict`, `DeferredPayload`, `DeferredRule`, and `DEFERRED_SCHEMA_VERSION` (independent of `Verdict::SCHEMA_VERSION`).
- New helper `hector_core::llm::prompt::build_evaluator_input(rules, primary, context)` — concatenates the (system, user) tuple from `build_prompt_split` for inclusion in the envelope's `_evaluator_input` field.
- Wire format documented in [`docs/emit-semantic-payload.md`](docs/emit-semantic-payload.md).
- **Library-additive only.** No `Verdict` change, no exit-code change. Existing direct-API users (anthropic / openrouter / ollama) are unaffected.
```

- [ ] **Step 7.2.2: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): record H1 deferred-payload path"
```

### Task 7.3: Archive the plan

**Files:**
- Move: `plans/2026-05-14-hector-h1-emit-semantic-payload.md` → `plans/archive/`
- Modify: `plans/README.md` (move H1 from the Future section to Archive)

- [ ] **Step 7.3.1: Move the plan**

```bash
git mv plans/2026-05-14-hector-h1-emit-semantic-payload.md plans/archive/
```

- [ ] **Step 7.3.2: Update plans/README.md**

In the Future section, edit the H1-H4 bullet to mark H1 shipped (or split the bullet). In the Archive section, add:

```markdown
- [`2026-05-14-hector-h1-emit-semantic-payload`](archive/2026-05-14-hector-h1-emit-semantic-payload.md) — `hector check --emit-semantic-payload` flag + `llm.provider: claude-code-subagent` provider arm + `DeferredVerdict` envelope (`hector_core::verdict_deferred`); enables H3 (Claude Code adapter subagent mode).
```

- [ ] **Step 7.3.3: Commit**

```bash
git add plans/
git commit -m "docs(plans): archive H1 emit-semantic-payload plan"
```

---

## Acceptance criteria check (against spec §H1)

| Spec criterion | Covered by |
|---|---|
| `--emit-semantic-payload` on file w/ only deterministic rules → standard verdict | `cli_e2e_emit_semantic_payload::flag_omitted_means_no_envelope` proves the standard shape; deterministic-only behaviour is the same path (no `deferred_rules` collected). Tightened to a dedicated test if needed. |
| Same flag on file with surviving semantic rules → `DeferredVerdict`, exit 0 | `cli_e2e_emit_semantic_payload::flag_emits_deferred_verdict_envelope` |
| `passed_checks` matches the rule IDs that ran-and-passed deterministically | Asserted in Phase 4's runner test once a deterministic rule is added — extend `runner_deferred_mode.rs` test to include a passing script rule and assert `deferred.passed_checks` contains its id |
| `_evaluator_input` byte-identical to direct-API path | `prompt::tests::build_evaluator_input_concatenates_split_prompt` |
| Deterministic block → exit 2, deferred path suppressed | `cli_e2e_emit_semantic_payload::deterministic_block_suppresses_deferred_envelope` |
| Flag does not enable LLM dispatch — `ANTHROPIC_API_KEY` present still emits envelope | Add a test in Phase 6 that sets `ANTHROPIC_API_KEY` env var via `assert_cmd::Command::env` and asserts the envelope is still emitted |

If any of these falls out of test coverage during implementation, add a task to close the gap before archiving.
