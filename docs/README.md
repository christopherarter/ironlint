# Hector documentation

Hector is a policy-enforcement pipeline for AI coding agents. You write rules in a `.hector.yml`; Hector checks each edit an agent makes and blocks the ones that break your policy.

New here? Start with [Getting started](getting-started.md) — you'll have a working rule gating real edits in a few minutes.

Want the big picture first? See the [Architecture diagram](architecture.md).

## Writing rules

The core of Hector. Pick an engine for the job, then write the rule.

- [Rules overview](writing-rules/README.md) — the anatomy of a rule and which engine to reach for
- [Running a shell check](writing-rules/shell-checks.md) — the `script` engine
- [Matching code structure](writing-rules/matching-code.md) — the `ast` engine
- [Asking an LLM to judge a change](writing-rules/asking-an-llm.md) — the `semantic` engine
- [Checking a whole edit session](writing-rules/whole-session-checks.md) — the `session` engine

## Configuring

- [Targeting files](configuring/targeting-files.md) — `scope:` globs and `skip:` patterns
- [Severity and disabling rules](configuring/severity-and-disabling.md) — `error` vs `warning`, and `hector-disable:` directives
- [Sharing config with `extends:`](configuring/inheritance.md) — inherit rules across repos
- [LLM providers](configuring/llm-providers.md) — Anthropic, OpenRouter, Ollama, Claude Code subagent
- [Baselines](configuring/baselines.md) — silence pre-existing violations

## Connecting your agent

- [Adapters overview](adapters/README.md) — what an adapter is and the fail-open contract
- [Claude Code](adapters/claude-code.md)
- [OpenCode](adapters/opencode.md)
- [Reasonix](../adapters/reasonix/README.md)
- [pi](../adapters/pi/README.md)

## Running and inspecting

- [Running checks](operating/running-checks.md) — `hector check`, exit codes, fail-closed
- [Inspecting your config](operating/inspecting-config.md) — `explain`, `guide`, `show-resolved-config`
- [Diagnostics](operating/diagnostics.md) — `hector doctor`
- [Telemetry](operating/telemetry.md) — the `.hector/log.jsonl` check log

## Trust and sandboxing

- [The trust gate](security/trust.md) — why Hector won't run an unsigned config
- [Capability sandboxing](security/capabilities.md) — network and write isolation for `script:` rules

## Reference

Lookup material. The guides above link here; you don't need to read it front to back.

- [CLI](reference/cli.md) — every command and flag
- [Config schema](reference/config-schema.md) — the full `.hector.yml` shape
- [Verdict JSON](reference/verdict-json.md) — the machine-readable verdict and exit codes
- [`show-resolved-config` output](reference/show-resolved-config.md) — the TSV/YAML/JSON contract
- [`--emit-semantic-payload`](reference/emit-semantic-payload.md) — the deferred-evaluation envelope
- [`record-verdict`](reference/record-verdict.md) — the adapter-internal telemetry writer
