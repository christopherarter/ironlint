# Telemetry — `.ironlint/log.jsonl`

IronLint appends newline-delimited JSON records to `.ironlint/log.jsonl` as checks run. The active file is owner-only (`0o600` on Unix), and downstream tools read it one line at a time. A file check produces one record; a repository sweep can produce one record per write-lifecycle file plus one batched record for pre-commit checks.

**Schema version:** `5`. This is a code constant (`telemetry::SCHEMA_VERSION`) that bumps when the record shape changes. It is **not** written into each line — there is no per-line version field.

## Discriminator

Every line carries a `type` field. The only value is `check`. Field names are `snake_case`.

## `check`

Written once per `ironlint check` invocation. It carries the lifecycle event, the verdict status, wall-clock elapsed, and a per-check outcome list.

```jsonl
{"type":"check","ts":"2026-06-28T12:00:01Z","file":"src/app.ts","event":"write","status":"block","elapsed_ms":42,"checks":[{"check":"no-console","status":"block","elapsed_ms":12},{"check":"biome","status":"pass","elapsed_ms":28}]}
{"type":"check","ts":"2026-06-28T12:00:02Z","set_size":3,"event":"pre-commit","status":"pass","elapsed_ms":30,"checks":[{"check":"no-todo","status":"pass","elapsed_ms":18}]}
```

| Field | Type | Description |
|---|---|---|
| `type` | `"check"` | Record discriminator. |
| `ts` | RFC3339 string | Wall-clock at the time the line was written. |
| `file` | string, optional | Path to the file checked. Present on `write` records; **omitted** on `pre-commit` (run-once) records, which have no single primary file. |
| `set_size` | integer, optional | Number of files in the checked set. Present on `pre-commit` records; **omitted** on per-file `write` records. |
| `event` | `"write"` \| `"pre-commit"` | The lifecycle that triggered the run. |
| `status` | `"pass"` \| `"block"` \| `"internal_error"` | Verdict status (matches `verdict.status`). |
| `elapsed_ms` | integer | Wall-clock for the whole check run. |
| `checks` | array of per-check records | One entry per check that ran. **Empty** when no check's `files` matched (the file was checked, but no check ran). |

### Per-check record

| Field | Type | Description |
|---|---|---|
| `check` | string | Check id from `.ironlint.yml`. |
| `step` | string, optional | The step name, when the check uses `steps:`. Omitted for a single-`run` check. |
| `status` | `"pass"` \| `"block"` \| `"internal_error"` | Outcome of this check. |
| `elapsed_ms` | integer | Wall-clock for this check's run. |
| `reason` | string, optional | Why the check crashed. Omitted on a plain pass or block; on an `internal_error` it's a stable string — `timeout`, `not_found`, `not_executable`, `signal:9`, `exit_code:137`. |

There is no warn status at either level, no `engine` field, and no `rule_id` — a check owns its verdict through its exit code, and IronLint logs the outcome it observed.

## Atomicity and concurrency

`telemetry::append` opens the file with `O_APPEND` and owner-only mode (`0o600`), takes an advisory `flock(LOCK_EX)`, writes one buffered line in a single `write_all`, then releases the lock. Concurrent `ironlint` invocations cannot interleave bytes: the kernel's `O_APPEND` atomicity covers writes below `PIPE_BUF`, and the `flock` covers larger lines.

## Rotation

After an append grows the active log beyond 10 MiB, IronLint renames it to `.ironlint/log.jsonl.1`. The next record creates a new active `log.jsonl`. IronLint keeps one archive: a later rotation replaces the existing `.1` file.

You may remove either file when you no longer need its history. A missing log is created on the next check run.
