# IronLint documentation

IronLint is a policy-enforcement layer for AI coding agents. You write **checks** in a `.ironlint.yml`; when an agent edits a file, IronLint runs the checks that match it and blocks the edits that break your policy.

A check is `files` plus a shell command (or a `steps` list) to run, with optional `on` and `name` fields:

```yaml
# .ironlint.yml
checks:
  no-console:
    files: "**/*.ts"
    run: "! grep -n 'console.log'"  # proposed content arrives on stdin
```

IronLint runs `run`, reads its exit code, and blocks the edit when the code is nonzero (1–125). That is the whole model — no engines, no severities, no rule DSL. The check owns the decision.

New here? Start with [Getting started](getting-started.md) — you'll have a check blocking a real edit in a few minutes.

Want the big picture first? See the [Visual elevator pitch](visual-elevator-pitch.md), then the [Architecture diagram](architecture.md).

## Writing checks

- [Anatomy of a check](writing-checks/README.md) — `files`, `run`, and the exit-code contract
- [Check recipes](writing-checks/recipes.md) — grep checks, linters over stdin, and whole-tree tools

## Configuring

- [Targeting files](configuring/targeting-files.md) — the `files:` globs each check matches
- [Disabling a check in-line](configuring/disabling.md) — `ironlint-disable:` directives
- [Sharing config with `extends:`](configuring/inheritance.md) — inherit checks across repos

## Connecting your agent

`ironlint init` detects your agents and wires the hook for you — start there, then reach for a page below for per-agent paths, scopes, and manual installs.

- [Adapters overview](adapters/README.md) — what an adapter is, the ABI it speaks, and the fail-open contract
- [Claude Code](adapters/claude-code.md)
- [OpenCode](adapters/opencode.md)
- [Codex](../adapters/codex/README.md)
- [pi](../adapters/pi/README.md)

## Running and inspecting

- [Running checks](operating/running-checks.md) — `ironlint check`, exit codes, fail-open
- [Inspecting your config](operating/inspecting-config.md) — `explain` and `show-resolved-config`
- [Diagnostics](operating/diagnostics.md) — `ironlint doctor`
- [Telemetry](operating/telemetry.md) — the `.ironlint/log.jsonl` check log

## Trust

- [The trust store](security/trust.md) — why IronLint won't run an unblessed config, and how `ironlint trust` works

## Reference

Lookup material. The guides above link here; you don't need to read it front to back.

- [CLI](reference/cli.md) — every command and flag
- [Config schema](reference/config-schema.md) — the full `.ironlint.yml` shape
- [Verdict JSON](reference/verdict-json.md) — the machine-readable verdict and exit codes
- [`show-resolved-config` output](reference/show-resolved-config.md) — the TSV/YAML/JSON contract
