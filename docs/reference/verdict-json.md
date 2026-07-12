# Verdict JSON

`ironlint check --format json` prints a `Verdict` — the machine-readable result a CI job or adapter consumes. This page is the contract for that shape and the exit codes that go with it.

```json
{
  "schema_version": 6,
  "ironlint_version": "0.11.0",
  "status": "block",
  "blocks": [
    {
      "check": "no-console",
      "step": null,
      "file": "src/app.ts",
      "message": "src/app.ts:12: console.log('debug')"
    }
  ],
  "errors": [],
  "passed": ["biome"],
  "elapsed_ms": 42
}
```

## Top-level fields

| Field | Type | Notes |
|-------|------|-------|
| `schema_version` | integer | Currently `6`. See [Versioning](#versioning). |
| `ironlint_version` | string | Version of the binary that produced the verdict. |
| `status` | enum | `pass`, `block`, or `internal_error`. |
| `blocks` | array | One [Block](#block) per check that blocked. Empty on a clean pass. |
| `errors` | array | One [GateError](#gateerror) per check that crashed. Empty when every check ran to a verdict. |
| `passed` | array of strings | Check ids that ran and passed. |
| `elapsed_ms` | integer | Wall-clock for the whole check run. |

There is no `violations` array, no `severity`, no `line`/`column`, and no warn status. A check owns its verdict through its exit code; IronLint records the outcome, not a parsed finding.

## `status`

| Value | Meaning |
|-------|---------|
| `pass` | No check blocked and none crashed. |
| `block` | At least one check blocked — exited nonzero (`1`–`125`). |
| `internal_error` | No check blocked, but at least one crashed (not found, not executable, timeout, or killed by signal). |

`block` wins over `internal_error`: a confirmed policy violation stops the edit even if an unrelated check crashed in the same run.

## Block

A check that blocked on a file.

| Field | Type | Notes |
|-------|------|-------|
| `check` | string | The check id that blocked. |
| `step` | string \| null | The step name that blocked, when the check uses `steps:`. `null` for a single-`run` check. |
| `file` | string \| null | Path the check blocked on. `null` for a `pre-commit` (run-once) check. |
| `message` | string | The check's combined stdout and stderr, trimmed and passed through verbatim. When both streams are empty, `"<check-id> blocked"`. |

## GateError

A check that failed to run to a verdict. (`GateError` is the type's name in the locked schema; its fields use the `check` vocabulary.)

| Field | Type | Notes |
|-------|------|-------|
| `check` | string | The check id that crashed. |
| `step` | string \| null | The step name that crashed, when the check uses `steps:`. `null` otherwise. |
| `file` | string \| null | Path it was checking. `null` for a `pre-commit` (run-once) check. |
| `reason` | string | A stable reason string (see below). |
| `detail` | string \| null | Human-readable remediation: names the run command (truncated to 80 characters) and, for timeouts, the effective timeout that fired. `null` when no detail is available (e.g. a synthetic error). Added in schema v6. |

`reason` is one of:

| Value | Cause |
|-------|-------|
| `not_found` | Exit `127` — the command was not found. |
| `not_executable` | Exit `126` — the command was found but not executable. |
| `timeout` | The check exceeded `execution.timeout_secs` and was killed. |
| `signal:<n>` | The check was killed by signal `n` (e.g. `signal:9`). |
| `exit_code:<n>` | A normal exit with code `n ≥ 128` (not a signal death). |
| `spawn:<message>` | The process could not be spawned at all. |

## Exit codes

The exit code mirrors the status — it is what scripts branch on without parsing JSON:

| Exit | Status | Meaning |
|------|--------|---------|
| `0` | `pass` | Every matched check passed. |
| `1` | — | Config or load error (parse failure, missing file, unknown `--check`). No verdict is produced. |
| `2` | `block` | At least one check blocked (exited nonzero). |
| `3` | `internal_error` | At least one check crashed. |
| `4` | — | Untrusted config or checks — run `ironlint trust`. Emitted by the trust gate *before* the engine loads or any check runs; no verdict is produced. |

Adapters fail-open on `3` by default; opt into fail-closed with `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`. Exit `4` is the opposite default: adapters must surface it loudly, and every pre-write adapter treats it as fail-closed and blocks the tool call. See [Running checks](../operating/running-checks.md) and [The trust store](../security/trust.md).

## Versioning

`schema_version` is `6` and bumps only on a breaking change — a field removal or rename, a type change, an enum-variant removal, or a re-interpretation of an existing field. Additive changes (a new optional field, a new enum variant) do not bump it. The v5→v6 bump for `GateError.detail` is a deliberate exception to that rule: `detail` is additive and nullable, but the readiness review flagged internal-error opacity as a contract gap consumers should be able to detect, so the bump signals "you may now rely on a remediation string being present (possibly `null`)." Future additive changes resume the no-bump default.

Consumers should range-check (`schema_version >= 6`) rather than hard-code `== 6`, so a future additive bump doesn't break them.

## See also

- [Running checks](../operating/running-checks.md) — producing and acting on verdicts
- [Telemetry](../operating/telemetry.md) — the on-disk check log, which has its own schema
