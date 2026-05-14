# Hector D1 — Typed Telemetry Records Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` (or `superpowers:subagent-driven-development`) to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Spec section:** [`specs/2026-05-12-bully-parity-closures.md` §D1](../specs/2026-05-12-bully-parity-closures.md)
**Severity:** 🟡 high (foundation for D2 `coverage` and D3 `debt`)
**Sequencing:** Track-D observability item; unblocks D2 + D3. No conflicts with the remaining 0.2.x cohort (A4 / E2 / C5 / F1) — telemetry is downstream of every engine, so each engine change can land independently.

---

## Goal

Replace the single flat `LogEntry` struct in `crates/hector-core/src/telemetry.rs` with a `serde(tag = "type", rename_all = "snake_case")` enum carrying four explicit variants — `SessionInit`, `Check`, `SemanticVerdict`, `SemanticSkipped` — each capturing the structure the consumer (D2 `coverage`, D3 `debt`, dashboards, log-greppers) actually needs. The existing flat shape stays readable for one release via a `#[serde(untagged)]` legacy reader so downgrade-and-roll-forward sequences don't lose data, and `hector_version` + `schema_version` start being stamped into every session so future analyzers can negotiate.

Bully wrote four record types from day one (`session_init`, `semantic_verdict`, `semantic_skipped`, file-level `check`); Hector has been overloading a single struct with a stringly-typed `kind` field plus optional `reason`. D2 and D3 cannot be cleanly written against today's shape — per-rule outcomes are missing, the `kind` discriminator is untyped, and there is no `session_init` anchor that ties later records to a hector binary version. D1 fixes those three things at once.

## Architecture

`crates/hector-core/src/telemetry.rs` grows from one struct to:

1. **`PerRuleRecord`** — `{ rule_id, engine, status, elapsed_ms, reason: Option<String> }`, the per-rule line that previously had no representation.
2. **`LogEntry`** — `serde(tag = "type", rename_all = "snake_case")` enum with the four variants from §D1. Public; serializes as `{"type":"check", ...}`.
3. **`LogEntryRead`** — private `serde(untagged)` wrapper that tries the typed enum first, then falls back to `LogEntryLegacy { timestamp, kind, file, rule_id, status, elapsed_ms, reason }`. Yields a `LogEntry` to the caller and emits a one-time stderr deprecation warning per process when it sees a legacy line. Ships now; removed at the 0.3 verdict freeze.
4. **`pub fn append(path, &LogEntry)`** — same write path as today (atomic single-write, exclusive `flock`, `0o600`), now routed through the enum's serde derive.
5. **`pub fn read_all(path) -> Result<Vec<LogEntry>>`** — new reader used by D2/D3 (and by the backwards-compat unit tests). Wraps the `LogEntryRead` deserialize and drops malformed lines with a warning rather than failing the whole batch.

`Status` and `Engine` from `crates/hector-core/src/verdict.rs` already derive `Serialize` with `#[serde(rename_all = "lowercase")]`. The telemetry enum **reuses those types directly** rather than restringifying — `verdict.status` and `Violation::engine` go straight into the record. This drops the four `format!("{:?}", verdict.status).to_lowercase()` sites in `runner.rs` and guarantees the on-disk strings can never diverge from the verdict JSON's strings. The wire-format strings stay `pass`/`warn`/`block` and `script`/`ast`/`semantic`/`session`/`trust`/`internal`.

`PerRuleRecord.engine` reuses `verdict::Engine` for the same reason. `PerRuleRecord.status` is also `verdict::Status` — `pass` for a clean rule, `warn`/`block` per the rule's severity if it fired (mirrors how the runner already maps severity → status). `PerRuleRecord.reason` is `Some("<SkipReason::as_str()>")` only for the explicit-skip path and is otherwise `None`. The skip-pattern record (currently `kind: "skipped"`) folds into a `Check` with empty `rules` — the file was checked, no rule ran, telemetry still anchors the wall-clock — and the `reason` field on `PerRuleRecord` is unused for that case (no rules at all). This keeps the variant count at four and matches bully's record types one-for-one.

`SessionInit` is written from a new helper, `commands/session.rs::start`, called from a new `hector session start` subcommand and lazily from `commands/session.rs::record` when no `session.json` exists yet (mirrors bully's "first edit triggers session_init"). The version comes from `env!("CARGO_PKG_VERSION")`.

## Tech Stack

Rust, workspace-stable. Already-direct deps on `hector-core`: `serde`, `serde_json`, `chrono`, `fs4`, `anyhow`. **No new deps.** `insta` (workspace dev-dep) is reused for the per-variant snapshots.

---

## Decisions ratified up-front

| Decision | Choice | Reason |
|---|---|---|
| Discriminator | `serde(tag = "type", rename_all = "snake_case")` on the enum | Spec §D1 step 1 verbatim. Matches bully's `type` field. |
| Variant set | `SessionInit`, `Check`, `SemanticVerdict`, `SemanticSkipped` | Four record types, one per bully telemetry class. `subagent_stop` is bully-host-specific; we map session checks into `Check` with `file: ""`. |
| `Status` serialization | Reuse `verdict::Status` directly in `Check.status` and `PerRuleRecord.status`. No new string mapping. | Single source of truth — the verdict's `lowercase` rename rule applies. Drops the four `format!("{:?}", …).to_lowercase()` sites. |
| `Engine` serialization | Reuse `verdict::Engine` directly in `PerRuleRecord.engine` | Same — single source of truth, `script`/`ast`/`semantic`/`session`/`trust`/`internal` strings inherited. |
| Skip-pattern (A2) record | Fold into `Check { rules: [] }` (no per-rule lines because no rule ran). The `file` field carries the path. | Simplifies the variant set. Consumers identify "skipped file" by `rules.is_empty()`. |
| Semantic-skipped (A3) record | `SemanticSkipped { ts, file, rule, reason }` — separate variant per spec | Bully has a distinct `semantic_skipped` type; D2 wants to count semantic-API costs avoided, which only this variant captures. The `reason` is the existing `SkipReason::as_str()` value (`empty` / `whitespace_only` / `comments_only` / `pure_deletion`). |
| Per-rule outcome (`PerRuleRecord.reason`) | `Option<String>`, populated only for engine-internal errors (`engine_error`) and `disable`-suppressed rows (`disabled`). `None` for vanilla pass/fire. | Keeps the field non-noisy. D3 doesn't need it for headline metrics; we still want it for explain-style debugging. |
| Q2 (A2 spec open question) | Settled in the spec: fold `Skipped` into `Pass` on the verdict; D1 carries the distinction in telemetry only. | Avoids a verdict-schema bump. The skip distinction lives in `Check { rules: [] }` (file-skip via A2 pattern) vs. `SemanticSkipped { … }` (per-rule semantic short-circuit via A3). Logged here so future-me doesn't re-litigate. |
| Backwards-compat reader | `#[serde(untagged)]` over `(LogEntry, LogEntryLegacy)`. One-time stderr deprecation warning per process via `OnceLock<()>`. | Mirrors the v1→v2 dance from `Baseline` in [E1](archive/2026-05-12-hector-e1-baseline-checksum.md); same shape, same lifetime (deprecation window = until 0.3 verdict freeze, removed in the same PR). |
| Deprecation window | One release. Legacy reader removed in the 0.3 freeze PR. | Spec §D1 acceptance criterion 3 verbatim ("Old `log.jsonl` files still parse during the deprecation window"). 0.3 is the natural inflection — verdict shape locks then; telemetry shape locks alongside it. |
| Writer always emits v2 | `LogEntry` is `Serialize`-only on the public type; `LogEntryRead` is `Deserialize`-only and private. The writer cannot accidentally produce a legacy line. | Matches the E1 split. Forces every newly-written record into the typed shape. |
| Public-API impact | `pub enum LogEntry` is a breaking change to the library surface. | Acceptable pre-1.0 (per `specs/overview.md` §7 — verdict locks at 0.3, library API has no stability promise yet). CHANGELOG entry is a task. |
| `hector_version` source | `env!("CARGO_PKG_VERSION")` | Same approach as `Verdict::pass` and `Verdict::from_violations`. Compile-time, zero overhead. |
| `schema_version` for telemetry | New constant `crate::telemetry::SCHEMA_VERSION: u32 = 1`. Independent of the verdict's `SCHEMA_VERSION`. | Telemetry and verdict can evolve on different cadences. The first typed-telemetry shape is v1; bumps when fields are added/removed. |
| Reader on malformed line | Drop the line with a `eprintln!` warning, continue parsing. Returns the well-formed lines. | A single corrupt line in a 100k-line log shouldn't fail D2/D3. Mirrors the runner's behavior on malformed baseline files. |
| Test fixture for legacy reader | Verbatim 5-line capture from a current-main `.hector/log.jsonl`, checked into `crates/hector-core/tests/fixtures/log_legacy.jsonl`. | Pins the wire shape that production hectors are producing today. |
| `cargo insta` snapshots | One per variant. Kept under `crates/hector-core/tests/snapshots/` (default `insta` location). | Doubles as the documentation source for `docs/telemetry.md`. |

---

## Risk / rollback

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Verdict shape impact | none | n/a | Verdict unchanged. `Status` / `Engine` / `Severity` / `SCHEMA_VERSION` are reused, not modified. |
| Exit-code contract impact | none | n/a | Telemetry is downstream of the verdict; exit codes derive from verdict status, not telemetry. |
| Telemetry shape impact | breaking | medium | `#[serde(untagged)]` legacy reader carries the old flat shape across one release. CHANGELOG entry. The legacy reader's deprecation warning fires once per process, surfacing the migration to operators. |
| Public-API impact | breaking | low | `pub enum LogEntry` replaces `pub struct LogEntry`. Pre-1.0; `hector-core` is consumed only by `hector-cli` in this workspace, and external crates aren't promised API stability yet. CHANGELOG documents the break. |
| Performance impact | low | low | Per-rule records expand JSONL line count by ~N rules/file. With a typical 5–20-rule config and ~50 files/check, that's ~250–1000 lines per check. `.hector/log.jsonl` already grows linearly with check count; this multiplies the constant by ~10. We do not rotate today; that stays a separate operability ticket. The single `flock`+`write_all` per record path is unchanged, so wall-clock impact per record is unchanged. |
| Q2 (A2) re-litigation | low | low | The spec settled on "fold `Skipped` into `Pass`; carry the distinction in telemetry." D1 inherits that. The `Check { rules: [] }` shape is the carrier; this row is the breadcrumb so future-me doesn't reopen the question. |
| Downgrade scenario (newer log read by older hector) | medium | low | Older hector's `serde_json::from_str::<LogEntryLegacy>` will fail on `{"type":"check", …}` lines. D2/D3 don't exist yet so consumers are limited. Operators who downgrade lose readability of new lines until they roll forward; old lines remain readable both directions. Documented in CHANGELOG. |
| Cognitive complexity in `read_all` line loop | medium | low | Keep the per-line decision tree to a single `match` on the `LogEntryRead` deserialize result. If clippy flags it, extract `parse_one(line) -> Option<LogEntry>` so the loop body stays under 15. |

**Rollback:** `git revert` of the squash-merge. The legacy reader path means existing `.hector/log.jsonl` files written under d1 will not parse under pre-d1 hector — operators rolling back will see corrupt-line warnings on logs from d1, and either truncate or accept the noise. Acceptable; same posture as E1.

---

## File structure

```
crates/hector-core/
├── src/
│   ├── telemetry.rs                         ← MODIFIED: enum + PerRuleRecord + read_all + legacy reader
│   ├── runner.rs                            ← MODIFIED: 4 call sites — `Check`, `SemanticSkipped`, skip-folded `Check`, session-aggregate `Check`
│   └── lib.rs                               ← unchanged (`pub mod telemetry;` already exposes the new types)
└── tests/
    ├── telemetry.rs                         ← MODIFIED: rewrite existing tests to the new shape; add round-trip tests per variant; add backward-compat test
    ├── telemetry_legacy.rs                  ← NEW: integration of the legacy fixture through `read_all`
    ├── snapshots/                           ← NEW: one `.snap` per variant from insta
    └── fixtures/
        └── log_legacy.jsonl                 ← NEW: verbatim 5-line capture of pre-D1 telemetry

crates/hector-cli/
├── src/
│   ├── cli.rs                               ← MODIFIED: add `Session::Start` subcommand
│   ├── main.rs                              ← MODIFIED: dispatch `Session::Start`
│   └── commands/session.rs                  ← MODIFIED: add `start` fn + emit SessionInit lazily from `record`
└── tests/
    └── cli_typed_telemetry.rs               ← NEW: end-to-end `hector check` → assert all four record types in the log

docs/
└── telemetry.md                             ← NEW: per-variant JSON schema + samples

CHANGELOG.md                                 ← MODIFIED: D1 entry under unreleased
```

Touch-site enumeration (every existing call to `telemetry::append` — confirmed via `grep -rn 'telemetry::append' crates/`):

| # | File | Line (current) | Today | After D1 |
|---|---|---|---|---|
| 1 | `crates/hector-core/src/runner.rs` | 527 | `LogEntry { kind: "skipped", file, status: "pass", … }` (skip-pattern A2 path) | `LogEntry::Check { ts, file, status: Status::Pass, elapsed_ms, rules: vec![] }` |
| 2 | `crates/hector-core/src/runner.rs` | 638 | `LogEntry { kind: "check", file, status: "<status>", … }` (per-file post-loop) | `LogEntry::Check { ts, file, status: verdict.status, elapsed_ms, rules: <PerRuleRecord-per-evaluated-rule> }` |
| 3 | `crates/hector-core/src/runner.rs` | 735 | `LogEntry { kind: "semantic_skipped", file, rule_id: Some(…), reason: Some(…), … }` (A3 path) | `LogEntry::SemanticSkipped { ts, file, rule, reason }` |
| 4 | `crates/hector-core/src/runner.rs` | 814 | `LogEntry { kind: "check_session", file: "", … }` (session aggregate) | `LogEntry::Check { ts, file: "".into(), status: verdict.status, elapsed_ms, rules: <PerRuleRecord-per-session-rule> }` |
| 5 | `crates/hector-cli/src/commands/session.rs` (new) | n/a | (no telemetry written today) | `LogEntry::SessionInit { ts, hector_version, schema_version }` from `start` and lazily from `record` |

`crates/hector-core/src/engine/semantic.rs` and `crates/hector-core/src/engine/session.rs` do **not** write telemetry directly today. The runner is the single writer, and we keep it that way — `SemanticVerdict` is also emitted from the runner inside the `Check` flow (one `PerRuleRecord` per rule, plus a `SemanticVerdict` line for every semantic rule that reached dispatch). Centralizing all writes in the runner keeps the parallel-dispatch ordering predictable and removes the need to thread the log path through the engine trait.

---

## Phase 1 — Define the typed enum + writer (no call sites yet)

The first phase introduces the new types behind a feature-isolated API, with the writer routed through them, but leaves the runner using the old shape via `#[allow(deprecated)]` shims. This lets us land the type machinery and snapshots in a small reviewable diff before touching any caller.

### Task 1: Failing per-variant round-trip tests

**Files:**
- Modify: `crates/hector-core/tests/telemetry.rs`

- [ ] **Step 1: Append the new tests at the bottom of the file.**

```rust
// --- D1: typed telemetry --------------------------------------------------

use hector_core::telemetry::{LogEntry, PerRuleRecord, SCHEMA_VERSION as TELEMETRY_SCHEMA};
use hector_core::verdict::{Engine, Status};

#[test]
fn session_init_round_trips() {
    let entry = LogEntry::SessionInit {
        ts: "2026-05-13T12:00:00Z".into(),
        hector_version: "0.2.2".into(),
        schema_version: TELEMETRY_SCHEMA,
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.contains("\"type\":\"session_init\""), "discriminator field present: {line}");
    assert!(line.contains("\"hector_version\":\"0.2.2\""));
    assert!(line.contains("\"schema_version\":1"));
    let back: LogEntry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry);
}

#[test]
fn check_round_trips_with_per_rule_records() {
    let entry = LogEntry::Check {
        ts: "2026-05-13T12:00:01Z".into(),
        file: "src/lib.rs".into(),
        status: Status::Pass,
        elapsed_ms: 12,
        rules: vec![
            PerRuleRecord {
                rule_id: "no-unwrap".into(),
                engine: Engine::Semantic,
                status: Status::Pass,
                elapsed_ms: 8,
                reason: None,
            },
            PerRuleRecord {
                rule_id: "no-todo".into(),
                engine: Engine::Script,
                status: Status::Warn,
                elapsed_ms: 4,
                reason: None,
            },
        ],
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.contains("\"type\":\"check\""), "discriminator: {line}");
    assert!(line.contains("\"rules\":["));
    assert!(line.contains("\"engine\":\"semantic\""));
    assert!(line.contains("\"engine\":\"script\""));
    let back: LogEntry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry);
}

#[test]
fn check_with_zero_rules_round_trips_and_marks_a_skipped_file() {
    // A2 skip-pattern fold: file was checked, no rule ran.
    let entry = LogEntry::Check {
        ts: "2026-05-13T12:00:02Z".into(),
        file: "Cargo.lock".into(),
        status: Status::Pass,
        elapsed_ms: 0,
        rules: vec![],
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.contains("\"rules\":[]"), "empty rules array preserved: {line}");
    let back: LogEntry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry);
}

#[test]
fn semantic_verdict_round_trips() {
    let entry = LogEntry::SemanticVerdict {
        ts: "2026-05-13T12:00:03Z".into(),
        rule: "no-secrets".into(),
        verdict: "pass".into(),
        file: Some("src/auth.rs".into()),
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.contains("\"type\":\"semantic_verdict\""));
    assert!(line.contains("\"file\":\"src/auth.rs\""));
    let back: LogEntry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry);
}

#[test]
fn semantic_verdict_with_no_file_round_trips() {
    let entry = LogEntry::SemanticVerdict {
        ts: "2026-05-13T12:00:04Z".into(),
        rule: "session-rule".into(),
        verdict: "pass".into(),
        file: None,
    };
    let line = serde_json::to_string(&entry).unwrap();
    let back: LogEntry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry);
}

#[test]
fn semantic_skipped_round_trips() {
    let entry = LogEntry::SemanticSkipped {
        ts: "2026-05-13T12:00:05Z".into(),
        file: "src/lib.rs".into(),
        rule: "no-unwrap".into(),
        reason: "pure_deletion".into(),
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.contains("\"type\":\"semantic_skipped\""));
    assert!(line.contains("\"reason\":\"pure_deletion\""));
    let back: LogEntry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry);
}

#[test]
fn snake_case_field_names_match_spec() {
    // Spec §D1 says fields are snake_case. Pin against accidental rename.
    let entry = LogEntry::SessionInit {
        ts: "t".into(),
        hector_version: "x".into(),
        schema_version: 1,
    };
    let value: serde_json::Value = serde_json::to_value(&entry).unwrap();
    let obj = value.as_object().unwrap();
    assert!(obj.contains_key("type"));
    assert!(obj.contains_key("ts"));
    assert!(obj.contains_key("hector_version"));
    assert!(obj.contains_key("schema_version"));
    assert!(!obj.contains_key("hectorVersion"), "must be snake_case, not camelCase");
    assert!(!obj.contains_key("hector-version"));
}
```

- [ ] **Step 2: Run, confirm failure.**

Run: `cargo test -p hector-core --test telemetry 2>&1 | tail -40`

Expected: compile errors. Exact stderr fragment:
- `error[E0432]: unresolved import `hector_core::telemetry::PerRuleRecord``
- `error[E0432]: unresolved import `hector_core::telemetry::SCHEMA_VERSION``
- and: `error[E0599]: no variant or associated item named `SessionInit` found for struct `LogEntry``

- [ ] **Step 3: Commit failing tests.**

```bash
git add crates/hector-core/tests/telemetry.rs
git commit -m "$(cat <<'EOF'
test(telemetry): failing tests for typed LogEntry enum (D1 phase 1)
EOF
)"
```

---

### Task 2: Replace the struct with the enum, keep `append` working

**Files:**
- Modify: `crates/hector-core/src/telemetry.rs`

- [ ] **Step 1: Rewrite the module body.**

```rust
//! Append-only check log at `.hector/log.jsonl`.
//!
//! D1: typed records. Every line is one `LogEntry`. The discriminator is
//! `type`, snake_case to match the rest of the spec; payload fields are
//! variant-specific.
//!
//! **Backwards compat:** the [`read_all`] reader accepts the legacy flat
//! shape (`{ "kind": "...", "timestamp": "...", ... }`) during a
//! deprecation window that ends at the 0.3 verdict freeze, when this
//! fallback is removed. The writer cannot produce the legacy shape — only
//! the new enum is `Serialize`.
use crate::verdict::{Engine, Status};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

/// Telemetry record-set version. Independent of the verdict schema; bumps
/// when this enum's shape changes (added/removed variants or fields).
pub const SCHEMA_VERSION: u32 = 1;

/// Per-rule outcome line carried inside a [`LogEntry::Check`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerRuleRecord {
    pub rule_id: String,
    pub engine: Engine,
    pub status: Status,
    pub elapsed_ms: u64,
    /// Optional reason: `"engine_error"` for runtime failures,
    /// `"disabled"` for `hector-disable:`-suppressed rows. `None` for
    /// vanilla pass/fire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One line in `.hector/log.jsonl`.
///
/// Discriminator field is `type`; variant payload follows. `Check.rules`
/// is an empty vec when the file was short-circuited by an A2 skip
/// pattern (file was checked, no rule ran).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LogEntry {
    SessionInit {
        ts: String,
        hector_version: String,
        schema_version: u32,
    },
    Check {
        ts: String,
        file: String,
        status: Status,
        elapsed_ms: u64,
        rules: Vec<PerRuleRecord>,
    },
    SemanticVerdict {
        ts: String,
        rule: String,
        verdict: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        file: Option<String>,
    },
    SemanticSkipped {
        ts: String,
        file: String,
        rule: String,
        reason: String,
    },
}

/// Has the legacy-format deprecation warning been emitted in this process?
static LEGACY_WARNING_EMITTED: OnceLock<()> = OnceLock::new();

/// Pre-D1 flat shape. Read-only; never serialized.
#[derive(Deserialize)]
struct LogEntryLegacy {
    timestamp: String,
    kind: String,
    file: String,
    rule_id: Option<String>,
    status: String,
    elapsed_ms: u64,
    #[serde(default)]
    reason: Option<String>,
}

/// Wrapper deserializer: try the typed shape first, fall back to legacy.
/// Untagged means serde will pick whichever variant fully matches.
#[derive(Deserialize)]
#[serde(untagged)]
enum LogEntryRead {
    Typed(LogEntry),
    Legacy(LogEntryLegacy),
}

impl LogEntryLegacy {
    /// Lift a flat legacy record into the closest typed equivalent. The
    /// mapping is intentionally lossy in one direction — `kind: "check"`
    /// loses per-rule detail because the legacy format never carried
    /// `rules`. Consumers must be aware that legacy `Check` records have
    /// `rules: vec![]`.
    fn into_typed(self) -> LogEntry {
        match self.kind.as_str() {
            "semantic_skipped" => LogEntry::SemanticSkipped {
                ts: self.timestamp,
                file: self.file,
                rule: self.rule_id.unwrap_or_default(),
                reason: self.reason.unwrap_or_default(),
            },
            "semantic_verdict" => LogEntry::SemanticVerdict {
                ts: self.timestamp,
                rule: self.rule_id.unwrap_or_default(),
                verdict: self.status,
                file: if self.file.is_empty() { None } else { Some(self.file) },
            },
            // "check", "check_session", "skipped" all collapse here.
            // Status string is best-effort; missing/unknown → Pass.
            _ => LogEntry::Check {
                ts: self.timestamp,
                file: self.file,
                status: parse_status(&self.status),
                elapsed_ms: self.elapsed_ms,
                rules: Vec::new(),
            },
        }
    }
}

fn parse_status(s: &str) -> Status {
    match s {
        "warn" => Status::Warn,
        "block" => Status::Block,
        _ => Status::Pass,
    }
}

fn emit_legacy_warning(path: &Path) {
    if LEGACY_WARNING_EMITTED.set(()).is_ok() {
        eprintln!(
            "hector: warning — telemetry log at {} contains pre-D1 (flat) records; \
             reading them through the legacy fallback. The fallback will be removed \
             at the 0.3 freeze.",
            path.display()
        );
    }
}

/// Append one record. Atomic single-write; owner-only mode; advisory
/// `flock` to serialize concurrent writers (the kernel only guarantees
/// O_APPEND atomicity below `PIPE_BUF`, so we lock for safety).
pub fn append(path: &Path, entry: &LogEntry) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut opts = OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;

    let mut line = serde_json::to_string(entry)?;
    line.push('\n');

    #[cfg(unix)]
    {
        use fs4::fs_std::FileExt;
        FileExt::lock_exclusive(&file)?;
        let result = file.write_all(line.as_bytes());
        FileExt::unlock(&file)?;
        result?;
    }
    #[cfg(not(unix))]
    file.write_all(line.as_bytes())?;

    Ok(())
}

/// Read every record in `path`, accepting both v2 (typed) and v1 (legacy
/// flat) shapes. Malformed lines are warned to stderr and dropped — a
/// single corrupt line should not fail the whole batch.
pub fn read_all(path: &Path) -> Result<Vec<LogEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<LogEntryRead>(line) {
            Ok(LogEntryRead::Typed(entry)) => out.push(entry),
            Ok(LogEntryRead::Legacy(legacy)) => {
                emit_legacy_warning(path);
                out.push(legacy.into_typed());
            }
            Err(e) => {
                eprintln!(
                    "hector: warning — telemetry log {}:{} dropped (parse error: {e})",
                    path.display(),
                    i + 1
                );
            }
        }
    }
    Ok(out)
}
```

- [ ] **Step 2: Make existing tests in `tests/telemetry.rs` compile against the new shape.**

The pre-D1 tests at the top of `tests/telemetry.rs` construct `LogEntry { timestamp, kind, file, rule_id, status, elapsed_ms, reason }`. They need to be rewritten to construct typed variants. The `append_creates_log_and_writes_jsonl` test, the concurrent-writers test, the `0o600` test, the `log_entry_with_reason_serializes_field` test, the parentless-path test, the `errors_when_parent_uncreatable` test, and the `log_entry_without_reason_omits_field` test all touch the old struct.

Replacement table:

| Existing test name | Replacement |
|---|---|
| `append_creates_log_and_writes_jsonl` | Construct two `LogEntry::Check { rules: vec![] }`, assert `"\"type\":\"check\""` and the file paths appear. |
| `telemetry_append_is_atomic_under_concurrent_writers` | Construct `LogEntry::Check { ts: "t".into(), file: format!("file-{i}-{j}-{}", "x".repeat(8192)), status: Status::Pass, elapsed_ms: 0, rules: vec![] }`. Assertion is unchanged: every line is valid JSON. |
| `telemetry_append_creates_file_with_mode_0600` | Same body with `LogEntry::Check { ts: "t".into(), file: "f".into(), status: Status::Pass, elapsed_ms: 0, rules: vec![] }`. |
| `log_entry_with_reason_serializes_field` | Replace with `LogEntry::SemanticSkipped { ts: …, file: …, rule: …, reason: "whitespace_only".into() }`. Assert `"\"reason\":\"whitespace_only\""` and `"\"type\":\"semantic_skipped\""`. |
| `telemetry_append_with_parentless_path_returns_error` | Same body with a `LogEntry::Check { rules: vec![] }`. |
| `telemetry_append_errors_when_parent_uncreatable` | Same body with a `LogEntry::Check { rules: vec![] }`. |
| `log_entry_without_reason_omits_field` | Now redundant — `Check` doesn't have a `reason` field at all. Replace assertion: construct a `LogEntry::SemanticVerdict { file: None, … }`, assert the serialized line does not contain `"file"`. |

Concrete rewrite of `append_creates_log_and_writes_jsonl`:

```rust
#[test]
fn append_creates_log_and_writes_jsonl() {
    let dir = tempdir().unwrap();
    let log = dir.path().join(".hector/log.jsonl");
    let entry = LogEntry::Check {
        ts: "2026-05-11T18:00:00Z".into(),
        file: "src/foo.rs".into(),
        status: Status::Pass,
        elapsed_ms: 12,
        rules: vec![],
    };
    append(&log, &entry).unwrap();
    let content = std::fs::read_to_string(&log).unwrap();
    assert!(content.contains("\"type\":\"check\""));
    assert!(content.contains("\"src/foo.rs\""));

    let entry2 = LogEntry::Check {
        ts: "2026-05-11T18:00:05Z".into(),
        file: "src/bar.rs".into(),
        status: Status::Block,
        elapsed_ms: 22,
        rules: vec![],
    };
    append(&log, &entry2).unwrap();
    let content = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<_> = content.lines().collect();
    assert_eq!(lines.len(), 2);
}
```

- [ ] **Step 3: Run the telemetry tests, confirm green.**

Run: `cargo test -p hector-core --test telemetry`

Expected: all tests in the file pass — both the rewritten pre-D1 tests and the new D1 round-trip tests from Task 1.

- [ ] **Step 4: Run the full hector-core suite. It will fail at the call sites — that's fine.**

Run: `cargo build -p hector-core 2>&1 | tail -30`

Expected: compile errors at the four `runner.rs` call sites listed in the touch table — `LogEntry::Check`/`SemanticSkipped` not constructible with the old field set.

- [ ] **Step 5: Commit the type machinery only.**

```bash
git add crates/hector-core/src/telemetry.rs crates/hector-core/tests/telemetry.rs
git commit -m "$(cat <<'EOF'
feat(telemetry): typed LogEntry enum + read_all + legacy reader (D1 phase 1)

Replace the flat LogEntry struct with a serde(tag = "type") enum carrying
SessionInit / Check / SemanticVerdict / SemanticSkipped variants. Add
read_all() which accepts both the new and pre-D1 (flat) shapes via an
untagged wrapper; emit a one-time deprecation warning per process when
the legacy reader fires. The runner call sites still need to be updated
to the new shape — that's phase 2.
EOF
)"
```

---

## Phase 2 — Wire the runner to typed records

### Task 3: Update the four `runner.rs` call sites

**Files:**
- Modify: `crates/hector-core/src/runner.rs`

The runner currently writes the old flat shape at four sites (lines 527, 638, 735, 814 — see touch table). All four need to be rewritten in one commit because they share the same import surface. The skip-pattern site folds into a `Check { rules: vec![] }`. The semantic-skipped site (A3) becomes `SemanticSkipped`. The post-loop per-file site becomes `Check { rules: <PerRuleRecord per evaluated rule> }`. The session-aggregate site becomes `Check { file: "".into(), rules: <PerRuleRecord per session rule> }`.

Per-rule outcomes need to flow through `RuleOutcome` so we can build `PerRuleRecord`. Today `RuleOutcome` carries `violations`, `passed`, and `explain`. Add a fourth field `record: PerRuleRecord` populated unconditionally inside `evaluate_one_rule` and `merge_engine_outcome`, so the post-loop telemetry write has a `Vec<PerRuleRecord>` ready to drop into the `Check`.

- [ ] **Step 1: Failing test — assert the new on-disk shape.**

Append to `crates/hector-core/tests/runner_skip.rs`:

```rust
#[test]
fn skip_pattern_emits_typed_check_with_empty_rules() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        r#"schema_version: 2
rules:
  silly:
    description: "any"
    engine: semantic
    scope: ["**/*.lock"]
    severity: error
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
    // D1: skip-pattern record is a `Check` with empty `rules`.
    assert!(log.contains("\"type\":\"check\""), "telemetry must use typed shape; got:\n{log}");
    assert!(log.contains("\"rules\":[]"), "skip-pattern record must have empty rules:\n{log}");
    // No legacy `kind` field anywhere.
    assert!(!log.contains("\"kind\":\"skipped\""), "legacy `kind` must be gone:\n{log}");
}
```

And append to `crates/hector-core/tests/runner_semantic_prefilter.rs`:

```rust
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
        .with_llm(Box::new(CountingLlm { calls: calls.clone() }))
        .load(&cfg)
        .unwrap();
    engine
        .check(CheckInput::Diff { file, unified_diff: diff.to_string() })
        .unwrap();

    let log = std::fs::read_to_string(dir.path().join(".hector/log.jsonl")).unwrap();
    // D1: typed shape with `type` discriminator.
    assert!(log.contains("\"type\":\"semantic_skipped\""), "log:\n{log}");
    assert!(log.contains("\"reason\":\"whitespace_only\""), "log:\n{log}");
    assert!(log.contains("\"rule\":\"no-unwrap\""), "rule field, not rule_id; log:\n{log}");
    // Per-rule check still recorded.
    assert!(log.contains("\"type\":\"check\""), "per-file Check record present; log:\n{log}");
}
```

- [ ] **Step 2: Run, confirm failure.**

Run: `cargo test -p hector-core --test runner_skip --test runner_semantic_prefilter 2>&1 | tail -30`

Expected: failures with `"\"type\":\"check\""` not found in the log content (because runner still writes the flat shape).

- [ ] **Step 3: Edit `crates/hector-core/src/runner.rs`. Add a `record` field to `RuleOutcome` and emit the four new shapes.**

Augment `RuleOutcome`:

```rust
struct RuleOutcome {
    violations: Vec<Violation>,
    passed: Option<String>,
    explain: Option<RuleExplain>,
    /// D1: per-rule telemetry line. Always populated when the rule
    /// reached engine dispatch (or was short-circuited by A3); `None`
    /// when the rule was out-of-scope (won't appear in the Check.rules
    /// array, matches "rule didn't run for this file" semantics).
    record: Option<PerRuleRecord>,
}
```

Helper for severity → status mapping (used inside `merge_engine_outcome` and `apply_disables`):

```rust
fn rule_fired_status(severity: crate::config::Severity) -> Status {
    match severity {
        crate::config::Severity::Error => Status::Block,
        crate::config::Severity::Warning => Status::Warn,
    }
}
```

Add `use crate::telemetry::{LogEntry, PerRuleRecord};` and `use crate::verdict::Status;` at the top of the file (next to existing `use` lines).

In `evaluate_one_rule`, the out-of-scope early return becomes `record: None`. The A3-skipped early return populates `record` with `Status::Pass` and `reason: Some(reason.clone())`:

```rust
if let Some(reason) = self.try_semantic_skip(rule_id, rule, inputs.path, inputs.diff) {
    let explain = inputs.collect_explain.then(|| RuleExplain {
        rule_id: rule_id.to_string(),
        engine: rule.engine,
        outcome: ExplainOutcome::Skipped { reason: reason.clone() },
    });
    let record = Some(PerRuleRecord {
        rule_id: rule_id.to_string(),
        engine: engine_kind_to_verdict_engine(rule.engine),
        status: Status::Pass,
        elapsed_ms: 0,
        reason: Some(reason),
    });
    return RuleOutcome { violations: vec![], passed: Some(rule_id.to_string()), explain, record };
}
```

Where `engine_kind_to_verdict_engine` is a tiny private helper (free of cognitive complexity):

```rust
fn engine_kind_to_verdict_engine(kind: EngineKind) -> crate::verdict::Engine {
    match kind {
        EngineKind::Script => crate::verdict::Engine::Script,
        EngineKind::Ast => crate::verdict::Engine::Ast,
        EngineKind::Semantic => crate::verdict::Engine::Semantic,
        EngineKind::Session => crate::verdict::Engine::Session,
    }
}
```

Wrap the engine dispatch with an `Instant::now()` so `elapsed_ms` is real:

```rust
let rule_start = Instant::now();
let outcome: Result<Vec<Violation>> = match rule.engine { /* unchanged */ };
let rule_elapsed = rule_start.elapsed().as_millis() as u64;
Self::merge_engine_outcome(rule_id, rule.engine, inputs, outcome, rule_elapsed)
```

`merge_engine_outcome` gets a new `elapsed: u64` parameter and threads it into the `record` it constructs:

```rust
fn merge_engine_outcome(
    rule_id: &str,
    engine: EngineKind,
    inputs: &CheckInputs<'_>,
    outcome: Result<Vec<Violation>>,
    elapsed: u64,
) -> RuleOutcome {
    let verdict_engine = engine_kind_to_verdict_engine(engine);
    match outcome {
        Ok(vs) if vs.is_empty() => {
            let explain = inputs.collect_explain.then(|| RuleExplain {
                rule_id: rule_id.to_string(),
                engine,
                outcome: explain_outcome_for(engine, false, false),
            });
            RuleOutcome {
                violations: vec![],
                passed: Some(rule_id.to_string()),
                explain,
                record: Some(PerRuleRecord {
                    rule_id: rule_id.to_string(),
                    engine: verdict_engine,
                    status: Status::Pass,
                    elapsed_ms: elapsed,
                    reason: None,
                }),
            }
        }
        Ok(vs) => Self::apply_disables(rule_id, engine, inputs, vs, elapsed),
        Err(e) => {
            let v = Violation { /* unchanged */ };
            let explain = inputs.collect_explain.then(|| RuleExplain {
                rule_id: rule_id.to_string(),
                engine,
                outcome: explain_outcome_for(engine, true, false),
            });
            RuleOutcome {
                violations: vec![v],
                passed: None,
                explain,
                record: Some(PerRuleRecord {
                    rule_id: rule_id.to_string(),
                    engine: verdict_engine,
                    status: Status::Block,
                    elapsed_ms: elapsed,
                    reason: Some("engine_error".into()),
                }),
            }
        }
    }
}
```

`apply_disables` similarly takes `elapsed` and sets `record.status` based on whether anything fired (`Status::Pass` if everything was disabled or nothing fired, `rule_fired_status(rule.severity)` if any kept). Since `apply_disables` doesn't currently see `rule.severity`, pass it explicitly — adjust the call site in `merge_engine_outcome` to forward `&inputs.path` is unchanged; we just need the severity. Lookup is cheap: walk the kept violations and read `kept.first().map(|v| v.severity)`. If kept is non-empty, every kept violation shares the rule's severity, so `kept[0].severity` is sufficient:

```rust
fn apply_disables(
    rule_id: &str,
    engine: EngineKind,
    inputs: &CheckInputs<'_>,
    vs: Vec<Violation>,
    elapsed: u64,
) -> RuleOutcome {
    let mut kept: Vec<Violation> = Vec::new();
    let mut any_emitted = false;
    let mut any_disabled = false;
    for v in vs {
        let disabled = match v.line {
            Some(line) => inputs.disable_map.is_disabled(line, rule_id),
            None => inputs.disable_map.is_disabled_file_wide(rule_id),
        };
        if disabled {
            any_disabled = true;
            continue;
        }
        kept.push(v);
        any_emitted = true;
    }
    let verdict_engine = engine_kind_to_verdict_engine(engine);
    let (status, reason) = if any_emitted {
        let sev = kept[0].severity;
        let s = match sev {
            crate::verdict::Severity::Error => Status::Block,
            crate::verdict::Severity::Warning => Status::Warn,
        };
        (s, None)
    } else if any_disabled {
        (Status::Pass, Some("disabled".to_string()))
    } else {
        (Status::Pass, None)
    };
    let passed = if any_emitted { None } else { Some(rule_id.to_string()) };
    let explain = inputs.collect_explain.then(|| RuleExplain {
        rule_id: rule_id.to_string(),
        engine,
        outcome: explain_outcome_for(engine, false, any_emitted),
    });
    RuleOutcome {
        violations: kept,
        passed,
        explain,
        record: Some(PerRuleRecord {
            rule_id: rule_id.to_string(),
            engine: verdict_engine,
            status,
            elapsed_ms: elapsed,
            reason,
        }),
    }
}
```

The collection loop in `check_inner` now also collects records:

```rust
let mut records: Vec<PerRuleRecord> = Vec::new();
for outcome in outcomes {
    violations.extend(outcome.violations);
    if let Some(id) = outcome.passed {
        passed.push(id);
    }
    if let Some(row) = outcome.explain {
        explain.push(row);
    }
    if let Some(rec) = outcome.record {
        records.push(rec);
    }
}
```

The skip-pattern (line 527) write becomes:

```rust
if let Err(e) = crate::telemetry::append(
    &self.config_dir.join(".hector/log.jsonl"),
    &LogEntry::Check {
        ts: chrono::Utc::now().to_rfc3339(),
        file: path.display().to_string(),
        status: Status::Pass,
        elapsed_ms: elapsed,
        rules: vec![],
    },
) {
    eprintln!("hector: telemetry append failed: {e:#}");
}
```

The post-loop write (line 638) becomes:

```rust
if let Err(e) = crate::telemetry::append(
    &self.config_dir.join(".hector/log.jsonl"),
    &LogEntry::Check {
        ts: chrono::Utc::now().to_rfc3339(),
        file: path.display().to_string(),
        status: verdict.status,
        elapsed_ms: verdict.elapsed_ms,
        rules: records,
    },
) {
    eprintln!("hector: telemetry append failed: {e:#}");
}
```

The semantic-skipped write inside `try_semantic_skip` (line 735) becomes:

```rust
let entry = LogEntry::SemanticSkipped {
    ts: chrono::Utc::now().to_rfc3339(),
    file: path.display().to_string(),
    rule: rule_id.to_string(),
    reason: reason_str.clone(),
};
```

The session-aggregate write (line 814) builds per-rule records inline as `check_session` walks `self.config.rules`. Add a local `let mut records: Vec<PerRuleRecord> = Vec::new();` at the top of the loop body in `check_session`, push one `PerRuleRecord { rule_id, engine: Engine::Session, status, elapsed_ms: rule_elapsed, reason }` per session rule (`status` from the per-rule outcome), and emit the wrapping `LogEntry::Check { file: "".into(), rules: records, … }` after the loop.

For each session rule that reaches the LLM and gets a `Pass`, also emit a `LogEntry::SemanticVerdict { ts, rule, verdict: "pass".into(), file: None }` immediately after the per-rule `evaluate` returns. For `Violation`, emit `LogEntry::SemanticVerdict { … verdict: "violation".into(), file: None }`. This satisfies the spec's "writes `SemanticVerdict` on pass/violation" wiring at the only remaining touch point that crosses an LLM boundary.

Symmetrically, in `evaluate_one_rule`, immediately after `crate::engine::semantic::SemanticEngine.run(&ctx)` returns (still inside the `Semantic` arm), emit a `LogEntry::SemanticVerdict { ts, rule: rule_id.to_string(), verdict: <pass|violation>, file: Some(path.display().to_string()) }`. Use a tiny helper to keep the cognitive complexity of `evaluate_one_rule` from spiking:

```rust
fn append_semantic_verdict(&self, rule_id: &str, file: Option<&str>, verdict_str: &str) {
    let entry = LogEntry::SemanticVerdict {
        ts: chrono::Utc::now().to_rfc3339(),
        rule: rule_id.to_string(),
        verdict: verdict_str.into(),
        file: file.map(str::to_string),
    };
    if let Err(e) = crate::telemetry::append(&self.config_dir.join(".hector/log.jsonl"), &entry) {
        eprintln!("hector: telemetry append failed: {e:#}");
    }
}
```

The semantic dispatch arm calls this with `verdict_str = if outcome.as_ref().map(|v| v.is_empty()).unwrap_or(false) { "pass" } else { "violation" }` based on the engine's `Result<Vec<Violation>>`. Keep the call inside `evaluate_one_rule` so it sits next to the semantic dispatch.

- [ ] **Step 4: Run the full hector-core suite.**

Run: `cargo test -p hector-core 2>&1 | tail -40`

Expected: green. The two failing tests from Step 1 pass. Existing tests that grep for `\"kind\":\"check\"` need rewriting if any remain — search and update:

```bash
grep -rn '\\"kind\\":' crates/hector-core/tests/ crates/hector-cli/tests/
```

Each hit is rewritten to grep for `"\"type\":"<variant>"\""`. The `cli_runner_telemetry_failure.rs` test does not grep for kind — it just asserts that an error is reported on stderr — so it stays as-is.

- [ ] **Step 5: Run the full workspace.**

Run: `cargo test 2>&1 | tail -20`

Expected: green.

- [ ] **Step 6: Commit.**

```bash
git add crates/hector-core/src/runner.rs crates/hector-core/tests/runner_skip.rs crates/hector-core/tests/runner_semantic_prefilter.rs
git commit -m "$(cat <<'EOF'
feat(runner): emit typed telemetry records at every write site (D1 phase 2)

Wire all four runner telemetry sites — skip-pattern (A2), per-file post-loop,
semantic-skipped (A3), and session-aggregate — to the typed LogEntry enum.
RuleOutcome gains a per-rule PerRuleRecord that the runner collects into
Check.rules. Semantic dispatch additionally emits SemanticVerdict on each
pass/violation, mirroring bully.
EOF
)"
```

---

## Phase 3 — `SessionInit` from the CLI

### Task 4: Add `hector session start` and emit `SessionInit`

**Files:**
- Modify: `crates/hector-cli/src/cli.rs`
- Modify: `crates/hector-cli/src/main.rs`
- Modify: `crates/hector-cli/src/commands/session.rs`

The CLI today exposes only `hector session record`. Add `hector session start` as a sibling that writes a single `SessionInit` line (idempotent — every invocation appends, mirroring bully's behavior of stamping every session boundary). Also have `record` emit `SessionInit` lazily when no `session.json` exists yet; that captures the normal adapter flow where the host calls `record` at the first edit without ever calling `start`.

- [ ] **Step 1: Failing CLI test.**

Create `crates/hector-cli/tests/cli_session_start.rs`:

```rust
use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn session_start_writes_session_init_telemetry() {
    let dir = tempdir().unwrap();
    let cfg_body = "schema_version: 2\nrules:\n  noop:\n    description: x\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n";
    let trusted = hector_core::trust::write_trust_block(cfg_body).unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(&cfg, trusted).unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .args(["session", "start", "--dir", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let log = fs::read_to_string(dir.path().join(".hector/log.jsonl")).expect("log");
    assert!(log.contains("\"type\":\"session_init\""), "log:\n{log}");
    assert!(log.contains("\"hector_version\":"), "version stamp present:\n{log}");
    assert!(log.contains("\"schema_version\":1"), "telemetry schema present:\n{log}");
}

#[test]
fn session_record_lazy_emits_session_init_on_first_edit() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "session", "record",
            "--dir", dir.path().to_str().unwrap(),
            "--file", "src/foo.rs",
            "--diff", "--- a/src/foo.rs\n+++ b/src/foo.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n",
        ])
        .assert()
        .success();

    let log = fs::read_to_string(dir.path().join(".hector/log.jsonl")).expect("log");
    assert!(
        log.contains("\"type\":\"session_init\""),
        "first record without prior session.json must emit session_init; log:\n{log}"
    );
}
```

- [ ] **Step 2: Run, confirm failure.**

Run: `cargo test -p hector-cli --test cli_session_start 2>&1 | tail -20`

Expected: `error: unrecognized subcommand 'start'` or `the following required arguments were not provided`, depending on clap's exact error path. Either way, the subcommand doesn't exist yet.

- [ ] **Step 3: Add the subcommand.**

In `crates/hector-cli/src/cli.rs`, augment `SessionAction`:

```rust
#[derive(Debug, Subcommand)]
pub enum SessionAction {
    /// Append an edit record to .hector/session.json.
    Record {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        file: PathBuf,
        #[arg(long, allow_hyphen_values = true)]
        diff: String,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// Stamp a `session_init` record into the telemetry log.
    Start {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
}
```

In `crates/hector-cli/src/main.rs`, add the dispatch arm. The current arm reads `SessionAction::Record { … } => commands::session::record(…)`; add:

```rust
SessionAction::Start { dir } => commands::session::start(&dir)?,
```

In `crates/hector-cli/src/commands/session.rs`, add a `start` function and wire `record` to lazily emit `SessionInit` when no prior `session.json` exists:

```rust
use hector_core::telemetry::{append as append_telemetry, LogEntry, SCHEMA_VERSION as TELEMETRY_SCHEMA};

pub fn start(dir: &Path) -> Result<i32> {
    let log = dir.join(".hector/log.jsonl");
    let entry = LogEntry::SessionInit {
        ts: chrono::Utc::now().to_rfc3339(),
        hector_version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version: TELEMETRY_SCHEMA,
    };
    if let Err(e) = append_telemetry(&log, &entry) {
        eprintln!("hector: telemetry append failed: {e:#}");
    }
    Ok(0)
}
```

Inside `record`, immediately after the `if state_path.exists() { … } else { … }` block decides we're creating a fresh state, also emit a `SessionInit`:

```rust
let mut state = if state_path.exists() {
    SessionState::load(&state_path)?
} else {
    let id = session_id.unwrap_or_else(|| format!("session-{}", chrono::Utc::now().timestamp()));
    // First edit of a session — stamp a session_init alongside the
    // session.json creation, matching bully's behavior. Failures here
    // are best-effort: the edit record is the source of truth.
    let log = hector_dir.join("log.jsonl");
    if let Err(e) = append_telemetry(
        &log,
        &LogEntry::SessionInit {
            ts: chrono::Utc::now().to_rfc3339(),
            hector_version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: TELEMETRY_SCHEMA,
        },
    ) {
        eprintln!("hector: telemetry append failed: {e:#}");
    }
    SessionState::new(id)
};
```

- [ ] **Step 4: Run.**

Run: `cargo test -p hector-cli --test cli_session_start`

Expected: green.

- [ ] **Step 5: Run the full workspace.**

Run: `cargo test`

Expected: green.

- [ ] **Step 6: Commit.**

```bash
git add crates/hector-cli/src/cli.rs crates/hector-cli/src/main.rs crates/hector-cli/src/commands/session.rs crates/hector-cli/tests/cli_session_start.rs
git commit -m "$(cat <<'EOF'
feat(cli): hector session start + lazy SessionInit on first record (D1 phase 3)

Adds `hector session start` which stamps a SessionInit telemetry record
with hector_version + schema_version. Also emits SessionInit lazily from
`session record` when no session.json exists yet, matching bully's
behavior of emitting session_init at the first edit.
EOF
)"
```

---

## Phase 4 — Backwards compatibility fixture and integration test

### Task 5: Pin the legacy wire shape with a checked-in fixture

**Files:**
- Create: `crates/hector-core/tests/fixtures/log_legacy.jsonl`
- Create: `crates/hector-core/tests/telemetry_legacy.rs`

The fixture is a verbatim 5-line capture of what current-main hector writes to `.hector/log.jsonl` (post-A3, pre-D1). It pins the exact wire shape we're claiming compatibility with so a future "small refactor to telemetry" can't silently drop it.

- [ ] **Step 1: Write the fixture.**

```jsonl
{"timestamp":"2026-05-12T18:00:00Z","kind":"check","file":"src/foo.rs","rule_id":null,"status":"pass","elapsed_ms":12}
{"timestamp":"2026-05-12T18:00:01Z","kind":"check","file":"src/bar.rs","rule_id":null,"status":"warn","elapsed_ms":4}
{"timestamp":"2026-05-12T18:00:02Z","kind":"semantic_skipped","file":"src/lib.rs","rule_id":"no-unwrap","status":"pass","elapsed_ms":0,"reason":"whitespace_only"}
{"timestamp":"2026-05-12T18:00:03Z","kind":"skipped","file":"Cargo.lock","rule_id":null,"status":"pass","elapsed_ms":0}
{"timestamp":"2026-05-12T18:00:04Z","kind":"check_session","file":"","rule_id":null,"status":"block","elapsed_ms":120}
```

- [ ] **Step 2: Write the integration test.**

```rust
use hector_core::telemetry::{read_all, LogEntry};
use hector_core::verdict::Status;
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn legacy_log_jsonl_loads_and_lifts_to_typed_variants() {
    let entries = read_all(&fixture_path("log_legacy.jsonl")).expect("legacy fixture must load");
    assert_eq!(entries.len(), 5, "all 5 legacy lines must lift, none dropped");

    // Line 1: kind=check → Check{rules:[]}
    match &entries[0] {
        LogEntry::Check { file, status, rules, .. } => {
            assert_eq!(file, "src/foo.rs");
            assert_eq!(*status, Status::Pass);
            assert!(rules.is_empty(), "legacy check has no per-rule data");
        }
        other => panic!("entry 0 should be Check, got {other:?}"),
    }

    // Line 3: kind=semantic_skipped → SemanticSkipped
    match &entries[2] {
        LogEntry::SemanticSkipped { file, rule, reason, .. } => {
            assert_eq!(file, "src/lib.rs");
            assert_eq!(rule, "no-unwrap");
            assert_eq!(reason, "whitespace_only");
        }
        other => panic!("entry 2 should be SemanticSkipped, got {other:?}"),
    }

    // Line 4: kind=skipped → Check{rules:[]}
    match &entries[3] {
        LogEntry::Check { file, rules, .. } => {
            assert_eq!(file, "Cargo.lock");
            assert!(rules.is_empty());
        }
        other => panic!("entry 3 should be Check, got {other:?}"),
    }

    // Line 5: kind=check_session → Check{file:"", rules:[]}
    match &entries[4] {
        LogEntry::Check { file, status, rules, .. } => {
            assert_eq!(file, "");
            assert_eq!(*status, Status::Block);
            assert!(rules.is_empty());
        }
        other => panic!("entry 4 should be Check, got {other:?}"),
    }
}

#[test]
fn malformed_legacy_line_is_dropped_with_warning() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log.jsonl");
    let body = "\
{\"timestamp\":\"t\",\"kind\":\"check\",\"file\":\"a\",\"rule_id\":null,\"status\":\"pass\",\"elapsed_ms\":1}
{not valid json
{\"timestamp\":\"t\",\"kind\":\"check\",\"file\":\"b\",\"rule_id\":null,\"status\":\"pass\",\"elapsed_ms\":2}
";
    std::fs::write(&log, body).unwrap();
    let entries = read_all(&log).expect("read_all must succeed even with a bad line");
    assert_eq!(entries.len(), 2, "the malformed line is dropped, the others survive");
}

#[test]
fn read_all_returns_empty_for_missing_log() {
    let dir = tempfile::tempdir().unwrap();
    let entries = read_all(&dir.path().join("nope.jsonl")).expect("missing file is empty, not error");
    assert!(entries.is_empty());
}
```

- [ ] **Step 3: Run.**

Run: `cargo test -p hector-core --test telemetry_legacy`

Expected: green.

- [ ] **Step 4: Commit.**

```bash
git add crates/hector-core/tests/fixtures/log_legacy.jsonl crates/hector-core/tests/telemetry_legacy.rs
git commit -m "$(cat <<'EOF'
test(telemetry): pin legacy wire shape via fixture (D1 phase 4)

Five-line capture of pre-D1 .hector/log.jsonl content; read_all() must
lift each line into the closest typed variant. Includes malformed-line
and missing-file coverage.
EOF
)"
```

---

### Task 6: End-to-end CLI integration test

**Files:**
- Create: `crates/hector-cli/tests/cli_typed_telemetry.rs`

This test runs `hector check` against a fixture project that exercises every variant: a pass-through script rule (Check + PerRuleRecord), a semantic rule against a pure-deletion diff (SemanticSkipped), and a `session start` (SessionInit). Asserts every variant appears in the log.

- [ ] **Step 1: Write the test.**

```rust
//! D1: end-to-end assertion that a realistic `hector` session writes every
//! typed-telemetry variant to .hector/log.jsonl.

use assert_cmd::Command;
use hector_core::telemetry::{read_all, LogEntry};
use std::fs;
use tempfile::tempdir;

fn write_trusted_config(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let cfg = dir.join(".hector.yml");
    let trusted = hector_core::trust::write_trust_block(body).unwrap();
    fs::write(&cfg, trusted).unwrap();
    cfg
}

#[test]
fn full_session_emits_every_typed_variant() {
    let dir = tempdir().unwrap();

    // 1) `hector session start` → SessionInit.
    Command::cargo_bin("hector")
        .unwrap()
        .args(["session", "start", "--dir", dir.path().to_str().unwrap()])
        .assert()
        .success();

    // 2) `hector check --file foo.txt` → Check with PerRuleRecord.
    let cfg_body = "schema_version: 2\nrules:\n  always-pass:\n    description: x\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n";
    let cfg = write_trusted_config(dir.path(), cfg_body);
    let target = dir.path().join("foo.txt");
    fs::write(&target, "hello\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--file", target.to_str().unwrap(),
            "--config", cfg.to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .assert()
        .success();

    // 3) Read the log via the typed reader.
    let log = dir.path().join(".hector/log.jsonl");
    let entries = read_all(&log).expect("log readable");

    let has_session_init = entries.iter().any(|e| matches!(e, LogEntry::SessionInit { .. }));
    let has_check_with_rule = entries.iter().any(|e| matches!(
        e, LogEntry::Check { rules, .. } if rules.iter().any(|r| r.rule_id == "always-pass")
    ));
    assert!(has_session_init, "missing SessionInit; entries: {entries:#?}");
    assert!(
        has_check_with_rule,
        "missing Check carrying PerRuleRecord{{rule_id:always-pass}}; entries: {entries:#?}"
    );
}

#[test]
fn semantic_skipped_record_is_emitted_for_pure_deletion_diff() {
    let dir = tempdir().unwrap();

    // Semantic rule with "avoid" phrasing → A3 short-circuits pure-deletion
    // diffs without dispatching to the LLM.
    let cfg_body = r#"schema_version: 2
llm:
  provider: anthropic
  model: claude-haiku
  api_key_env: HECTOR_FAKE_KEY
rules:
  no-todo:
    description: "avoid TODO comments"
    engine: semantic
    scope: ["*.rs"]
    severity: warning
"#;
    let cfg = write_trusted_config(dir.path(), cfg_body);

    let target = dir.path().join("foo.rs");
    fs::write(&target, "fn main() {}\n").unwrap();

    let diff_path = dir.path().join("change.diff");
    fs::write(
        &diff_path,
        "--- a/foo.rs\n+++ b/foo.rs\n@@ -1,2 +1,1 @@\n fn main() {}\n-let x = 1;\n",
    )
    .unwrap();

    // Expected to short-circuit before hitting the LLM (pure deletion + "avoid").
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--diff", diff_path.to_str().unwrap(),
            "--config", cfg.to_str().unwrap(),
        ])
        .env("HECTOR_FAKE_KEY", "x") // never read; LLM never dispatched
        .current_dir(dir.path())
        .assert()
        .success();

    let log = dir.path().join(".hector/log.jsonl");
    let entries = read_all(&log).expect("log readable");
    let has_skip = entries.iter().any(|e| matches!(
        e, LogEntry::SemanticSkipped { rule, reason, .. }
        if rule == "no-todo" && reason == "pure_deletion"
    ));
    assert!(has_skip, "missing SemanticSkipped{{pure_deletion}}; entries: {entries:#?}");
}
```

- [ ] **Step 2: Run.**

Run: `cargo test -p hector-cli --test cli_typed_telemetry`

Expected: green. If `semantic_skipped_record_is_emitted_for_pure_deletion_diff` fails because the diff doesn't classify as pure-deletion (an extra hunk gets parsed as added context), drop the leading-context line so the diff becomes `@@ -1,1 +0,0 @@\n-let x = 1;\n`. The unit tests in `crates/hector-core/src/diff/analysis.rs` document the exact classifier.

- [ ] **Step 3: Commit.**

```bash
git add crates/hector-cli/tests/cli_typed_telemetry.rs
git commit -m "$(cat <<'EOF'
test(cli): end-to-end typed-telemetry assertion (D1 phase 4)

Drives `hector session start` + `hector check` against a fixture project
and asserts SessionInit, Check(rules:[PerRuleRecord]), and
SemanticSkipped variants all land in .hector/log.jsonl.
EOF
)"
```

---

## Phase 5 — `insta` snapshots (one per variant)

### Task 7: Pin each variant's serialized form via `insta`

**Files:**
- Modify: `crates/hector-core/tests/telemetry.rs` (append `insta` snapshot tests)
- Create: `crates/hector-core/tests/snapshots/telemetry__*.snap` (auto-generated)

The snapshots double as the documentation source for `docs/telemetry.md` (Task 8). Each snapshot pins the exact wire shape so an accidental field rename surfaces as a CI failure rather than a silent telemetry break.

- [ ] **Step 1: Append to `tests/telemetry.rs`.**

```rust
// --- D1: insta snapshots, one per variant ---------------------------------

use insta::assert_json_snapshot;

#[test]
fn snapshot_session_init() {
    let entry = LogEntry::SessionInit {
        ts: "2026-05-13T12:00:00Z".into(),
        hector_version: "0.2.2".into(),
        schema_version: 1,
    };
    assert_json_snapshot!(entry);
}

#[test]
fn snapshot_check_with_rules() {
    let entry = LogEntry::Check {
        ts: "2026-05-13T12:00:01Z".into(),
        file: "src/lib.rs".into(),
        status: Status::Warn,
        elapsed_ms: 42,
        rules: vec![
            PerRuleRecord {
                rule_id: "no-unwrap".into(),
                engine: Engine::Semantic,
                status: Status::Pass,
                elapsed_ms: 30,
                reason: None,
            },
            PerRuleRecord {
                rule_id: "no-todo".into(),
                engine: Engine::Script,
                status: Status::Warn,
                elapsed_ms: 4,
                reason: None,
            },
        ],
    };
    assert_json_snapshot!(entry);
}

#[test]
fn snapshot_check_skip_pattern() {
    let entry = LogEntry::Check {
        ts: "2026-05-13T12:00:02Z".into(),
        file: "Cargo.lock".into(),
        status: Status::Pass,
        elapsed_ms: 0,
        rules: vec![],
    };
    assert_json_snapshot!(entry);
}

#[test]
fn snapshot_semantic_verdict() {
    let entry = LogEntry::SemanticVerdict {
        ts: "2026-05-13T12:00:03Z".into(),
        rule: "no-secrets".into(),
        verdict: "pass".into(),
        file: Some("src/auth.rs".into()),
    };
    assert_json_snapshot!(entry);
}

#[test]
fn snapshot_semantic_skipped() {
    let entry = LogEntry::SemanticSkipped {
        ts: "2026-05-13T12:00:04Z".into(),
        file: "src/lib.rs".into(),
        rule: "no-unwrap".into(),
        reason: "pure_deletion".into(),
    };
    assert_json_snapshot!(entry);
}
```

- [ ] **Step 2: Run with `INSTA_UPDATE=auto`, then review.**

Run: `INSTA_UPDATE=auto cargo test -p hector-core --test telemetry snapshot_`

Then inspect with: `cargo insta review`

Accept all five generated `.snap` files. Each snapshot must contain the variant's `type` discriminator and only the variant's expected fields. Specific shape for `snapshot_session_init`:

```json
{
  "type": "session_init",
  "ts": "2026-05-13T12:00:00Z",
  "hector_version": "0.2.2",
  "schema_version": 1
}
```

- [ ] **Step 3: Re-run without `INSTA_UPDATE` to confirm pinning.**

Run: `cargo test -p hector-core --test telemetry snapshot_`

Expected: green. The snapshots are now pinned.

- [ ] **Step 4: Commit.**

```bash
git add crates/hector-core/tests/telemetry.rs crates/hector-core/tests/snapshots/
git commit -m "$(cat <<'EOF'
test(telemetry): insta snapshots, one per variant (D1 phase 5)

Snapshots pin the exact wire shape of each typed LogEntry variant so an
accidental field rename surfaces as a CI failure. Snapshots double as the
documentation source for docs/telemetry.md.
EOF
)"
```

---

## Phase 6 — Documentation + CHANGELOG

### Task 8: Write `docs/telemetry.md`

**Files:**
- Create: `docs/telemetry.md`

- [ ] **Step 1: Write the doc.**

````markdown
# Telemetry — `.hector/log.jsonl`

Hector appends one JSON record per line to `.hector/log.jsonl` for every check it performs. The file is owner-only (`0o600`) and append-only — Hector never rewrites or truncates it. Operators rotate it themselves; downstream tools (`hector coverage`, `hector debt`, dashboards, log greppers) read it line-by-line.

**Schema version:** `1`. Stamped into every `session_init` record. Bumps when this enum's shape changes (added or removed variants/fields).

**Compatibility:** `hector` 0.2.2+ writes the typed shape documented below. Hectors before 0.2.2 wrote a flat shape (`{ "timestamp": ..., "kind": ..., ... }`). The current reader (`hector_core::telemetry::read_all`) accepts both; it lifts each legacy line into the closest typed variant and emits a one-time stderr deprecation warning. The legacy reader is removed at the 0.3 verdict freeze.

## Discriminator

Every record carries a `type` field. Values: `session_init`, `check`, `semantic_verdict`, `semantic_skipped`. Field names are `snake_case`.

---

## `session_init`

Stamped at the start of every session — either explicitly via `hector session start`, or lazily at the first `hector session record` when no `session.json` exists yet. Anchors all subsequent records to a hector binary version.

```json
{
  "type": "session_init",
  "ts": "2026-05-13T12:00:00Z",
  "hector_version": "0.2.2",
  "schema_version": 1
}
```

| Field | Type | Description |
|---|---|---|
| `type` | `"session_init"` | Record discriminator. |
| `ts` | RFC3339 string | Wall-clock at the time the record was written. |
| `hector_version` | string | Value of `CARGO_PKG_VERSION` of the writing binary. |
| `schema_version` | integer | Telemetry schema version. `1` at present. |

---

## `check`

Written once per `hector check` call against a single file (or once per `hector check --session` aggregate). Carries the verdict status, wall-clock elapsed, and a per-rule outcome list.

A check whose `rules` array is **empty** indicates one of two scenarios:
1. The file matched an A2 skip pattern (`Cargo.lock`, `node_modules/`, etc.); no rule ran.
2. Legacy upgrade path: a pre-D1 line was lifted into this shape because the flat format never carried per-rule detail.

Distinguish the two by reading earlier `session_init` records — fresh sessions only emit empty `rules` arrays for case 1.

```json
{
  "type": "check",
  "ts": "2026-05-13T12:00:01Z",
  "file": "src/lib.rs",
  "status": "warn",
  "elapsed_ms": 42,
  "rules": [
    {
      "rule_id": "no-unwrap",
      "engine": "semantic",
      "status": "pass",
      "elapsed_ms": 30
    },
    {
      "rule_id": "no-todo",
      "engine": "script",
      "status": "warn",
      "elapsed_ms": 4
    }
  ]
}
```

| Field | Type | Description |
|---|---|---|
| `type` | `"check"` | Record discriminator. |
| `ts` | RFC3339 string | Wall-clock at the time the record was written. |
| `file` | string | Path to the file checked. Empty string for `--session` aggregates. |
| `status` | `"pass"` \| `"warn"` \| `"block"` | Verdict status (matches `verdict.status`). |
| `elapsed_ms` | integer | Wall-clock for the whole check, including dispatch and baseline filter. |
| `rules[]` | array of `PerRuleRecord` | One entry per rule that reached engine dispatch (or was short-circuited by A3). Empty when an A2 skip pattern matched. |

**`PerRuleRecord`:**

| Field | Type | Description |
|---|---|---|
| `rule_id` | string | Rule id from `.hector.yml`. |
| `engine` | `"script"` \| `"ast"` \| `"semantic"` \| `"session"` \| `"trust"` \| `"internal"` | Engine that evaluated the rule. |
| `status` | `"pass"` \| `"warn"` \| `"block"` | Pass for clean evaluations and disable-suppressed; warn/block follows the rule's `severity` if it fired. |
| `elapsed_ms` | integer | Wall-clock for this rule's dispatch. `0` for short-circuited rules. |
| `reason` | string, optional | `"engine_error"` for runtime failures, `"disabled"` for `hector-disable:`-suppressed rows, A3 reason (`empty` / `whitespace_only` / `comments_only` / `pure_deletion`) for short-circuited rules. Omitted when there's nothing to say. |

---

## `semantic_verdict`

Written every time the semantic engine reaches the LLM and returns a verdict (pass or violation). One per semantic rule per dispatched evaluation. Used by D2 (`hector coverage`) to count semantic-API hits.

```json
{
  "type": "semantic_verdict",
  "ts": "2026-05-13T12:00:03Z",
  "rule": "no-secrets",
  "verdict": "pass",
  "file": "src/auth.rs"
}
```

| Field | Type | Description |
|---|---|---|
| `type` | `"semantic_verdict"` | Record discriminator. |
| `ts` | RFC3339 string | Wall-clock at the time the record was written. |
| `rule` | string | Rule id. |
| `verdict` | `"pass"` \| `"violation"` | The LLM's decision. |
| `file` | string, optional | Path of the file under check. Omitted for `--session` evaluations where there is no single file. |

---

## `semantic_skipped`

Written every time the A3 diff pre-filter short-circuits a semantic rule before dispatch. Lets D2 quantify the cost the local pre-filter avoided.

```json
{
  "type": "semantic_skipped",
  "ts": "2026-05-13T12:00:04Z",
  "file": "src/lib.rs",
  "rule": "no-unwrap",
  "reason": "pure_deletion"
}
```

| Field | Type | Description |
|---|---|---|
| `type` | `"semantic_skipped"` | Record discriminator. |
| `ts` | RFC3339 string | Wall-clock at the time the record was written. |
| `file` | string | Path of the file under check. |
| `rule` | string | Rule id. |
| `reason` | `"empty"` \| `"whitespace_only"` \| `"comments_only"` \| `"pure_deletion"` | Why the pre-filter decided not to dispatch. See `crates/hector-core/src/diff/analysis.rs`. |

---

## Atomicity and concurrency

`telemetry::append` opens with `O_APPEND`, takes an advisory `flock(LOCK_EX)`, writes one buffered line in a single `write_all`, then releases the lock. Concurrent `hector` invocations (e.g. parallel rules in a future B1 work-stealing pool) cannot interleave bytes. The kernel's `O_APPEND` atomicity guarantee covers writes below `PIPE_BUF`; the `flock` covers larger lines.

## Rotation

Hector does not rotate `.hector/log.jsonl` itself. Operators handle rotation. The append-only contract means external rotation (e.g. `logrotate copytruncate`) is safe — a missing-or-empty file is silently re-created on the next append.
````

- [ ] **Step 2: Cross-link from the `crates/hector-core/src/telemetry.rs` module doc.**

Add to the top-of-module doc comment in `telemetry.rs`:

```rust
//! Wire format documented in [`docs/telemetry.md`](../../docs/telemetry.md).
```

- [ ] **Step 3: Update `CHANGELOG.md`.**

Add a new section at the top:

```markdown
## Unreleased

### Telemetry — typed records (D1)

- `.hector/log.jsonl` now carries typed records: `session_init`, `check`, `semantic_verdict`, `semantic_skipped`. Each line has a `type` discriminator. Per-rule outcomes (`PerRuleRecord`) are nested under `Check.rules` instead of being one-line-per-(file,rule). `hector_version` and a telemetry `schema_version` are stamped in every `session_init`.
- **Backwards compat:** `hector_core::telemetry::read_all` accepts the pre-D1 flat shape via an untagged fallback and lifts each line into the closest typed variant. A one-time stderr deprecation warning fires per process when the fallback is used. The fallback will be removed at the 0.3 verdict freeze.
- New CLI subcommand `hector session start` stamps a `session_init` record explicitly. `hector session record` stamps one lazily on its first invocation per session.
- **Breaking (library):** `pub enum LogEntry` replaces `pub struct LogEntry` in `hector_core::telemetry`. Pre-1.0; consumers using the writer should migrate to constructing the appropriate variant.
- Wire format documented in [`docs/telemetry.md`](docs/telemetry.md).
```

- [ ] **Step 4: Commit.**

```bash
git add docs/telemetry.md CHANGELOG.md crates/hector-core/src/telemetry.rs
git commit -m "$(cat <<'EOF'
docs(telemetry): wire-format reference + CHANGELOG entry (D1 phase 6)

Documents every typed record variant, its fields, and the legacy-shape
deprecation window. Cross-linked from the telemetry module's doc comment.
EOF
)"
```

---

## Phase 7 — Coverage backfill, lint sweep

### Task 9: Coverage check on `telemetry.rs`

**Files:**
- Possibly modify: `crates/hector-core/tests/telemetry.rs` if any region is uncovered.

The CLAUDE.md gate is ≥80% region coverage per file. The new `telemetry.rs` has these decision points to land coverage on:

1. `LogEntryLegacy::into_typed` — three match arms (`semantic_skipped`, `semantic_verdict`, default `_`). All three exercised by the legacy fixture (Task 5).
2. `parse_status` — three match arms. Exercised by the legacy fixture's `pass`/`warn`/`block` mix.
3. `read_all` — file-missing branch (one test in Task 5), malformed-line branch (one test), happy-path typed-line branch (Task 6 e2e), happy-path legacy-line branch (Task 5).
4. `emit_legacy_warning` — `OnceLock::set` success branch (legacy fixture). Failure (already-set) branch reached by reading two legacy logs in the same process — exercise via a tiny test:

```rust
#[test]
fn legacy_warning_fires_only_once() {
    // Write the same legacy line to two distinct files. Read both.
    // Both reads must succeed; the warning is "best-effort" anyway, so
    // we only assert no panic and the expected entry counts.
    let dir = tempdir().unwrap();
    let a = dir.path().join("a.jsonl");
    let b = dir.path().join("b.jsonl");
    let line = "{\"timestamp\":\"t\",\"kind\":\"check\",\"file\":\"x\",\"rule_id\":null,\"status\":\"pass\",\"elapsed_ms\":1}\n";
    std::fs::write(&a, line).unwrap();
    std::fs::write(&b, line).unwrap();
    assert_eq!(read_all(&a).unwrap().len(), 1);
    assert_eq!(read_all(&b).unwrap().len(), 1);
}
```

- [ ] **Step 1: Run the coverage script.**

Run: `bash scripts/ci-coverage.sh 2>&1 | grep -E '(telemetry|FAIL|PASS)' | head -20`

Expected: `crates/hector-core/src/telemetry.rs` ≥80% region.

- [ ] **Step 2: If `telemetry.rs` is below 80%, identify the missing region.**

Run: `cargo llvm-cov --lib --workspace --fail-uncovered-regions 0 --html` and open the resulting HTML at `target/llvm-cov/html/index.html`. Add focused tests for any `parse_status`/`into_typed`/`read_all` branch the existing tests miss.

- [ ] **Step 3: Same check on `runner.rs`.**

The new code in `runner.rs` (the `engine_kind_to_verdict_engine` helper, the per-rule record construction in three sites, the `append_semantic_verdict` helper) needs region coverage. The existing `tests/runner_skip.rs`, `tests/runner_semantic_prefilter.rs`, and the new `cli_typed_telemetry.rs` exercise the happy paths. Add one test for the engine-error path if not already covered:

```rust
// In crates/hector-core/tests/runner_semantic_prefilter.rs:
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
        .check(CheckInput::File { path: target, content: "fn main(){}\n".into() })
        .unwrap();

    let entries = hector_core::telemetry::read_all(&dir.path().join(".hector/log.jsonl")).unwrap();
    let has_engine_error = entries.iter().any(|e| matches!(
        e, hector_core::telemetry::LogEntry::Check { rules, .. }
        if rules.iter().any(|r| r.reason.as_deref() == Some("engine_error"))
    ));
    assert!(has_engine_error, "missing engine_error per-rule record; entries: {entries:#?}");
}
```

- [ ] **Step 4: Run coverage again, confirm gate.**

Run: `bash scripts/ci-coverage.sh`

Expected: every file ≥80%. No regressions in pre-existing files.

- [ ] **Step 5: Commit any backfill tests.**

```bash
git add crates/hector-core/tests/telemetry.rs crates/hector-core/tests/runner_semantic_prefilter.rs
git commit -m "$(cat <<'EOF'
test(telemetry): coverage backfill for legacy reader + engine_error path (D1 phase 7)
EOF
)"
```

---

### Task 10: Final lint + format sweep

- [ ] **Step 1: `cargo fmt --check`.**

Run: `cargo fmt --check`

Expected: no diff.

- [ ] **Step 2: `cargo clippy --all-targets -- -D warnings`.**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -40`

Expected: green. Watch for cognitive-complexity warnings on `merge_engine_outcome`, `apply_disables`, and `check_inner` — each gained branches in this plan. If any function exceeds 15, extract the per-rule record construction into a small helper (e.g. `make_per_rule_record(rule_id, engine, status, elapsed, reason)`) before reaching for `#[allow(clippy::cognitive_complexity)]`. The reaching is documented in CLAUDE.md as a last resort, not a default.

- [ ] **Step 3: `cargo test --workspace`.**

Run: `cargo test --workspace 2>&1 | tail -10`

Expected: green.

- [ ] **Step 4: Mutation-testing spot-check (local, ad-hoc).**

Run: `cargo mutants --file crates/hector-core/src/telemetry.rs 2>&1 | tail -30`

Expected: zero surviving mutants in the new code paths. CLAUDE.md treats mutation testing as ad-hoc rather than CI-gated, but a survivor in code we just touched is a coverage gap — add a focused test for any survivor before continuing.

- [ ] **Step 5: Commit any sweep fixes.**

```bash
git add -A
git commit -m "$(cat <<'EOF'
style(telemetry): fmt + clippy sweep (D1 phase 7)
EOF
)"
```

---

## Phase 8 — Move plan to archive

### Task 11: Mark complete

- [ ] **Step 1:** Update `plans/README.md`: lift the D1 entry into the Archive section with a one-line summary; remove from Active.
- [ ] **Step 2:** `git mv plans/2026-05-13-hector-d1-typed-telemetry.md plans/archive/`.
- [ ] **Step 3:** Commit:

```bash
git add plans/README.md plans/archive/2026-05-13-hector-d1-typed-telemetry.md
git commit -m "$(cat <<'EOF'
docs(plans): archive D1 typed-telemetry plan
EOF
)"
```

---

## Test plan summary

| Test | File | Acceptance criterion covered |
|---|---|---|
| `session_init_round_trips` | `tests/telemetry.rs` | Variant 1 round-trips |
| `check_round_trips_with_per_rule_records` | `tests/telemetry.rs` | Variant 2 round-trips with `PerRuleRecord` nesting |
| `check_with_zero_rules_round_trips_and_marks_a_skipped_file` | `tests/telemetry.rs` | A2 skip-pattern fold |
| `semantic_verdict_round_trips` | `tests/telemetry.rs` | Variant 3 round-trips with `file: Some(_)` |
| `semantic_verdict_with_no_file_round_trips` | `tests/telemetry.rs` | `file: None` omits the field via `skip_serializing_if` |
| `semantic_skipped_round_trips` | `tests/telemetry.rs` | Variant 4 round-trips |
| `snake_case_field_names_match_spec` | `tests/telemetry.rs` | Spec §D1 field-naming pin |
| `skip_pattern_emits_typed_check_with_empty_rules` | `tests/runner_skip.rs` | Runner site 1 (skip-pattern) wired |
| `semantic_skipped_telemetry_uses_typed_variant` | `tests/runner_semantic_prefilter.rs` | Runner site 3 (A3 short-circuit) wired |
| `engine_error_yields_per_rule_record_with_engine_error_reason` | `tests/runner_semantic_prefilter.rs` | `PerRuleRecord.reason = "engine_error"` |
| `session_start_writes_session_init_telemetry` | `cli_session_start.rs` | New `hector session start` subcommand |
| `session_record_lazy_emits_session_init_on_first_edit` | `cli_session_start.rs` | Lazy `SessionInit` from `record` |
| `legacy_log_jsonl_loads_and_lifts_to_typed_variants` | `tests/telemetry_legacy.rs` | AC3: legacy reader round-trip |
| `malformed_legacy_line_is_dropped_with_warning` | `tests/telemetry_legacy.rs` | Reader resilience |
| `read_all_returns_empty_for_missing_log` | `tests/telemetry_legacy.rs` | Reader missing-file behavior |
| `legacy_warning_fires_only_once` | `tests/telemetry.rs` | `OnceLock` deprecation gate |
| `full_session_emits_every_typed_variant` | `cli_typed_telemetry.rs` | AC1: realistic session contains all four variants |
| `semantic_skipped_record_is_emitted_for_pure_deletion_diff` | `cli_typed_telemetry.rs` | A3 wired through CLI to telemetry |
| `snapshot_session_init` / `_check_with_rules` / `_check_skip_pattern` / `_semantic_verdict` / `_semantic_skipped` | `tests/telemetry.rs` (via `insta`) | AC2: each record validates against a documented schema |

---

## Acceptance-criteria checklist

Mapped to spec §D1's three acceptance bullets:

- [ ] **AC1 — `.hector/log.jsonl` contains all four record types in a realistic session.** Covered by `full_session_emits_every_typed_variant` (Task 6). Also implicitly by the round-trip + runner tests.
- [ ] **AC2 — Each record validates against a documented JSON schema (publish under `docs/telemetry.md`).** `docs/telemetry.md` written in Task 8; `insta` snapshots in Task 7 pin the wire shape against the doc.
- [ ] **AC3 — Old `log.jsonl` files still parse during the deprecation window.** `legacy_log_jsonl_loads_and_lifts_to_typed_variants` against the verbatim 5-line fixture (Task 5). One-time stderr deprecation warning per `OnceLock<()>` gate (Task 2). Fallback removed at the 0.3 freeze (documented in `docs/telemetry.md` and CHANGELOG).

Plus the corollaries the prose of §D1 implies:

- [ ] **Per-rule outcomes carried under `Check.rules`.** Task 3 wires `RuleOutcome.record` and the post-loop telemetry write builds the `Vec<PerRuleRecord>`.
- [ ] **`hector_version` + `schema_version` stamped at session start.** Task 4 (`SessionInit` carries both).
- [ ] **A3 reason wired through `SemanticSkipped.reason`.** Task 3 (semantic-skipped runner site).
- [ ] **A2 skip-pattern fold preserved (no separate variant).** Task 3 (skip-pattern runner site → `Check { rules: vec![] }`); covered by `skip_pattern_emits_typed_check_with_empty_rules`.
- [ ] **CHANGELOG entry for the breaking library-API change.** Task 8.

---

## Self-review checklist

(Per the writing-plans skill: walk the plan once, top to bottom, before handing off.)

- [ ] Every acceptance criterion has a named test in the test-plan table.
- [ ] Every code step shows full code; no placeholders, no "similar to above", no "TBD".
- [ ] Every test step shows the test in full and the exact `cargo test ...` invocation.
- [ ] Every failing-test step states the expected stderr fragment.
- [ ] Every commit step uses HEREDOC `git commit -m`.
- [ ] Touch-site enumeration in the architecture section names every existing `telemetry::append` call site.
- [ ] Q2 (A2) decision is explicitly inherited and recorded in Decisions Ratified to prevent re-litigation.
- [ ] Risk/rollback section enumerates verdict-schema impact (none), exit-code-contract impact (none), telemetry-schema impact (breaking; mitigated), public-API impact (breaking; pre-1.0).
- [ ] Coverage gate is named (`bash scripts/ci-coverage.sh`, ≥80% region per file) and a backfill task exists if any region is uncovered.
- [ ] Cognitive-complexity cap is named (15) and the at-risk functions (`merge_engine_outcome`, `apply_disables`, `check_inner`) are flagged with a refactor-not-allow first-line response.
- [ ] No new dependencies added — confirmed against `Cargo.toml` (Task header notes `serde`, `serde_json`, `chrono`, `fs4`, `anyhow` already direct deps; `insta` already a workspace dev-dep).
- [ ] Filename is `plans/2026-05-13-hector-d1-typed-telemetry.md` per CLAUDE.md / parity-spec §6 — **not** the writing-plans skill default of `docs/superpowers/plans/…`.
- [ ] Mutation-testing spot-check is opt-in/local (per CLAUDE.md), not CI-gated.
- [ ] Plan is moved to `archive/` only after merge (Task 11).

---

## Hand-off

- One worktree per phase is overkill; the eight phases are sequentially dependent (each commit unblocks the next). Run sequentially in a single worktree.
- Total tasks: **11** across **8 phases**.
- After merge, archive this plan to `plans/archive/` per the convention.
