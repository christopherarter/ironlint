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
(see [`Verdict::SCHEMA_VERSION`](#) bumped to `3`). The shape:

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

## Limitations (0.2.x)

- `--diff` combined with `--emit-semantic-payload` is rejected; multi-file
  envelope aggregation is a follow-up.
- The envelope assumes a single primary file. `engine: session` rules
  that span multiple changed files still produce one envelope; the
  subagent receives every session-rule definition but only the primary
  file/diff.
