# `hector check --emit-semantic-payload`

Adapter-internal flag for the Claude Code subagent path. When set, semantic
and session rules are collected into a `DeferredVerdict` JSON envelope
instead of being dispatched to the configured LLM.

Activated by either:
- `llm.provider: claude-code-subagent` in `.hector.yml` (end-user-facing),
- or the long-only `--emit-semantic-payload` CLI flag (adapter-internal,
  used for explicit invocations and tests).

## Envelope shape

`schema_version: 2` (bumped from `1` by R5 in 2026-05-23 for the optional
`payload.evaluator_model` field). Independent of `Verdict::SCHEMA_VERSION`.

```json
{
  "schema_version": 2,
  "deferred": true,
  "hector_version": "0.2.0",
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
    "_evaluator_input": "<TRUSTED_POLICY>…</UNTRUSTED_EVIDENCE>",
    "evaluator_model": "haiku"
  },
  "elapsed_ms": 42
}
```

### `payload.evaluator_model` (R5, optional)

Optional string. Mirrors `llm.evaluator_model` from `.hector.yml`. Surfaced
in the payload so the Claude Code interpreter skill can pick which model
to run the `hector-evaluator` subagent under (e.g. `haiku` for cheap
checks). The value is free-form — Claude Code rejects invalid model ids
at dispatch time, which is the right validation layer.

Serialized with `#[serde(skip_serializing_if = "Option::is_none")]`: when
the field is unset, the envelope is byte-compatible with the pre-R5 (v1)
shape, so downstream consumers that don't read the field do not break.
This is the only addition between schema v1 and v2.

## Exit-code semantics

| Outcome | Exit code | Stdout |
|---|---|---|
| Deterministic block | `2` | Standard `Verdict` (carries `deferred_rules` if any matched) |
| Pass + deferred non-empty | `0` | `DeferredVerdict` envelope |
| Pass + no deferred | `0` | Standard `Verdict` |

Deferred eval is not a block — the verdict is decided later by the
in-session subagent.

### Deferred rules on a blocked verdict (R6, 2026-05-23)

When a deterministic rule blocks (exit `2`) and an `--emit-semantic-payload`
run also had semantic/session rules in scope, the full deferred envelope
is suppressed — but the rules themselves surface on `Verdict.deferred_rules`
(`Verdict::SCHEMA_VERSION` remains `2` per the additive-no-bump policy — the field is optional and gated by `skip_serializing_if`). The shape:

```json
{
  "status": "block",
  "violations": [...],
  "deferred_rules": [
    {"rule_id": "no-todo-comment", "severity": "warning", "reason": "suppressed by deterministic block"}
  ]
}
```

`deferred_rules` is omitted entirely (via `skip_serializing_if = "Vec::is_empty"`)
when no semantic/session rule matched, so non-deferred-mode verdicts are
byte-compatible with the v2 shape. The Claude Code interpreter skill
surfaces these to the user so they can see their semantic rules are
configured but were not evaluated this turn — fixing the block and
re-triggering the hook will run them normally.

## Session-level envelopes (B3, 2026-05-26)

When `hector check --session --emit-semantic-payload` is invoked (or the
config has `llm.provider: claude-code-subagent`) and at least one
`engine: session` rule is in scope for at least one edit, the runner
emits a **session-level** `DeferredVerdict` instead of requiring an
`LlmClient`.

Session envelopes differ from per-file envelopes in two fields:

| Field | Per-file | Session |
|---|---|---|
| `payload.file` | `"src/foo.rs"` | `""` (empty — aggregates all in-scope edits) |
| `payload.diff` | unified diff of the changed file | framed aggregate of every in-scope edit |

### Aggregate framing

Each edit in `SessionState.edits` that matches at least one session rule's
scope is framed as:

```
<<<EDIT {session_id}/{file}>>>
{timestamp}
{diff}
<<<END EDIT>>>
```

Frames are joined with a blank line. The `session_id` in the delimiter
prevents attacker-controlled diff content from forging a frame boundary
for a different file (P1-9 principle carried forward from the LLM path).

### Claude Code stop hook

The stop hook detects `llm.provider: claude-code-subagent` via
`hector show-resolved-config` and passes `--emit-semantic-payload` to
`hector check --session`. When the result contains `deferred: true`, it
wraps `payload` in `hookSpecificOutput.additionalContext`:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "Stop",
    "additionalContext": "AGENTIC LINT SESSION EVALUATION REQUIRED:\n\n{payload JSON}"
  }
}
```

This mirrors the `PostToolUse` per-file branch exactly, so the
`hector-evaluator` subagent skill receives session rules through the
same channel.

## Limitations (0.2.x)

- `--diff` combined with `--emit-semantic-payload` is rejected; multi-file
  envelope aggregation is a follow-up.
- Per-file envelopes assume a single primary file. Session envelopes
  aggregate all in-scope edits via `framed_aggregate`.
