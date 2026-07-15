# Watching checks

`ironlint watch` gives you a live, read-only view of the check runs already
recorded in `.ironlint/log.jsonl`. Run it in a terminal beside your coding
agent when you want to see which checks are passing, blocking edits, or failing
to start:

```bash
ironlint watch
```

The command reads the log in the current project. To inspect another project,
pass the directory that contains its `.ironlint.yml`:

```bash
ironlint watch --dir ../service-api
```

`watch` requires an interactive terminal. It does not run checks, write
telemetry, or enforce trust.

## Following recent runs

The Stream view opens first. It lists the newest check runs first and updates
as new records arrive. A blocked row names the check that rejected the edit;
an internal-error row names the reason the check could not run. The original
block message is not stored in telemetry, so read it from the agent's tool
result when you need the full remediation.

Press `Tab`, `Right`, or `Left` to switch to the Explorer. Press `q` or `Esc`
to quit.

## Finding noisy or failing checks

The Explorer summarizes the whole active log: total runs, blocks, internal
errors, pass rate, and per-check median latency. It lists checks with blocks
before checks that only pass, so the checks that need attention stay visible.

Use `Up` and `Down` to select a check, then press `Enter` to return to a
Stream view filtered to that check. This is useful when you want to distinguish
a strict policy from a broken command or a check that never fires.

Telemetry rotates automatically after the active log exceeds 10 MiB. See
[Telemetry](telemetry.md) for the record schema and rotation behavior.
