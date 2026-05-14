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
| `file` | string | Path of the file under check. |
| `rule` | string | Rule id. |
| `reason` | `"empty"` \| `"whitespace_only"` \| `"comments_only"` \| `"pure_deletion"` | Why the pre-filter decided not to dispatch. See `crates/hector-core/src/diff/analysis.rs`. |

---

## Atomicity and concurrency

`telemetry::append` opens with `O_APPEND`, takes an advisory `flock(LOCK_EX)`, writes one buffered line in a single `write_all`, then releases the lock. Concurrent `hector` invocations (e.g. parallel rules in a future B1 work-stealing pool) cannot interleave bytes. The kernel's `O_APPEND` atomicity guarantee covers writes below `PIPE_BUF`; the `flock` covers larger lines.

## Rotation

Hector does not rotate `.hector/log.jsonl` itself. Operators handle rotation. The append-only contract means external rotation (e.g. `logrotate copytruncate`) is safe — a missing-or-empty file is silently re-created on the next append.
