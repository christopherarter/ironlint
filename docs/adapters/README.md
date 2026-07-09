# Adapters

An adapter wires IronLint into a coding agent so policy runs automatically on every edit, instead of you calling `ironlint check` by hand. The adapter hooks the agent's edit events, runs `ironlint check`, and translates the exit code into "allow" or "reject this edit."

**`ironlint init` installs these for you.** Run it with no arguments to detect every agent you have and wire them all at once; the per-agent command targets just one. The per-adapter pages below cover scopes, paths, and manual fallbacks.

| Adapter | Agent | Wire it in | Under the hood |
|---------|-------|------------|----------------|
| [Claude Code](claude-code.md) | Claude Code | `ironlint init --harness claude-code` | `PreToolUse` hook in `.claude/settings.local.json` (project-scoped, or `~/.claude/settings.json` with `--global`) |
| [OpenCode](opencode.md) | OpenCode | `ironlint init --harness opencode` | plugin file in `.opencode/plugins/` (project-scoped) |
| [Codex](../../adapters/codex/README.md) | OpenAI Codex | `ironlint init --harness codex` | `PreToolUse` hook in `.codex/hooks.json` (project-scoped, or `~/.codex/hooks.json` with `--global`) |
| [pi](../../adapters/pi/README.md) | pi | `ironlint init --harness pi` | extension in `.pi/extensions/` |

*Aider, pre-commit, and MCP adapters are planned.*

## What adapters do

The exact hook names and coverage differ, but the contract is the same across agents:

1. **On each edit or proposed edit** — collapse the host's hook payload into IronLint's ABI and run `ironlint check` against the file. On exit `2`, gating hooks reject the edit so the agent retries.
2. **On each Bash/shell command** — the agent's shell tool is gated too (`Bash` for claude-code/codex, `bash` for pi/opencode). Commands that would let the agent free itself — `ironlint trust`, or a Bash write to `.ironlint.yml` / `.ironlint/scripts/` — are denied via `ironlint gate-bash`, a separate built-in that is not a `check` and not trust-gated (it fires even with no `.ironlint.yml`). See the per-adapter "Bash gate" sections and `docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`.

Every adapter normalizes its host into the same ABI, so one check command runs unchanged everywhere:

| Channel | Value |
|---------|-------|
| `$IRONLINT_FILE` | Absolute path to the file under check. |
| `$IRONLINT_ROOT` | Project root — also the check's working directory. |
| `$IRONLINT_EVENT` | `write` or `pre-commit`. |
| `$IRONLINT_TMPFILE` | **write only** — set only when the check's `run` references it: absolute path to a temp file holding the proposed content, placed beside `$IRONLINT_FILE` with the same extension. Auto-cleaned after the check. Unset on `pre-commit`. |
| stdin | The proposed post-edit content. |

The adapter only shells out to the `ironlint` binary. It doesn't reimplement any policy logic.

## The exit-code contract

Adapters translate [`ironlint check`'s exit codes](../operating/running-checks.md) into agent actions:

| `ironlint` exit | Adapter action |
|---------------|----------------|
| `0` (pass) | Allow the edit. |
| `2` (block) | Reject the edit; the agent retries. |
| `1` (config error) | **Fail-open** — log and allow. An unrelated problem, like a broken config, shouldn't block the agent's work. |
| `3` (internal error) | **Fail-open by default** — log and allow. Set `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1` to make internal errors block where the host lifecycle can still block. |

The fail-open default on internal errors is deliberate: a rule that *couldn't run* is not a rule that *found a problem*. To make internal errors blocking instead — for strict CI-style enforcement — set `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`. See [Running checks](../operating/running-checks.md).

> **Codex is the one exception to this table.** Its `PreToolUse` hook doesn't block via exit code at all — it prints a `permissionDecision:"deny"` JSON object on stdout and exits `0`, and malformed stdout on a would-be block fails open. The codex adapter translates the same `ironlint check` exit codes above into that JSON internally; see the [Codex adapter](../../adapters/codex/README.md) for the exact contract.

## Timeout budget

IronLint's default per-check wall-clock cap is 30s (`execution.timeout_secs` in [the config schema](../reference/config-schema.md#execution)). Checks dispatch **sequentially** — one `run` invocation per matching file for `write`, one per check for `pre-commit` — so a file that matches `K` checks can take up to `K × timeout_secs` before ironlint reports a verdict.

Some JSON-hook harnesses impose their own timeout on the whole hook process, on top of that. If the harness's hook timeout is shorter than the worst-case sequential-check budget, the harness kills the hook before ironlint can report a verdict — and the edit lands **ungated**, with no verdict to fail open on. That's a silent bypass, not the fail-open behavior exit code `3` describes above.

Codex sets a hook-level timeout of its own; see the [Codex adapter](../../adapters/codex/README.md#timeout-budget) for the registered value and why. Claude Code's hook does not set one (see [its adapter page](claude-code.md#timeout-budget)).

If you raise `execution.timeout_secs` in your config, raise any harness-side hook timeout to match it — and re-run `ironlint init` for that harness if it's the one that set the timeout, so the regenerated hook entry picks up the new headroom.

## Requirements

Every adapter needs:

- the `ironlint` binary on `PATH`,
- a `.ironlint.yml` in the project root,
- a trusted config (`ironlint trust` — or let `ironlint init` scaffold and trust one).

If hooks aren't firing for any agent, run [`ironlint doctor`](../operating/diagnostics.md) — it reports one adapter row per agent (detected / installed / registered / intact), so you can see exactly which wiring is missing.

## Managing policy from inside the agent

Adapters that support skills ship three for managing policy without leaving the session:

- **`ironlint-config`** is the authoring guide: the `{files, run}` check schema, the
  exit-code contract, and the common patterns, with a fixture-test loop. `ironlint
  init` installs it as a real skill into every detected agent, and `ironlint schema`
  prints it on demand.
- **`/ironlint-init`** scaffolds a `.ironlint.yml` from your project's stack.
- **`/ironlint-review`** reads your telemetry log and reports which checks are noisy,
  which never fire, and which look dead.

`ironlint init` installs `ironlint-config` for every agent it wires (all four support
the Agent Skills spec). `/ironlint-init` and `/ironlint-review` ship with the Claude
Code plugin today; other agents gain them as their needs settle.

## See also

- [Claude Code adapter](claude-code.md)
- [OpenCode adapter](opencode.md)
