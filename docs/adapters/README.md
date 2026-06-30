# Adapters

An adapter wires Hector into a coding agent so policy runs automatically on every edit, instead of you calling `hector check` by hand. The adapter hooks the agent's edit events, runs `hector check`, and translates the exit code into "allow" or "reject this edit."

**`hector init` installs these for you.** Run it with no arguments to detect every agent you have and wire them all at once; the per-agent command targets just one. The per-adapter pages below cover scopes, paths, and manual fallbacks.

| Adapter | Agent | Wire it in | Under the hood |
|---------|-------|------------|----------------|
| [Claude Code](claude-code.md) | Claude Code | `hector init --harness claude-code` | `PostToolUse` hook in `.claude/settings.json` |
| [OpenCode](opencode.md) | OpenCode | `hector init --harness opencode` | plugin file in `.opencode/plugins/` (project-scoped) |
| [Reasonix](../../adapters/reasonix/README.md) | DeepSeek-Reasonix | `hector init --harness reasonix` | `PreToolUse` hook in `~/.reasonix/settings.json` (user-global) |
| [pi](../../adapters/pi/README.md) | pi | `hector init --harness pi` | extension in `.pi/extensions/` |

*Aider, pre-commit, and MCP adapters are planned.*

## What adapters do

The exact hook names and coverage differ, but the contract is the same across agents:

1. **On each edit or proposed edit** — collapse the host's hook payload into Hector's ABI and run `hector check` against the file. On exit `2`, gating hooks reject the edit so the agent retries.

Every adapter normalizes its host into the same ABI, so one check command runs unchanged everywhere:

| Channel | Value |
|---------|-------|
| `$HECTOR_FILE` | Absolute path to the file under check. |
| `$HECTOR_ROOT` | Project root — also the check's working directory. |
| `$HECTOR_EVENT` | `write` or `pre-commit`. |
| `$HECTOR_TMPFILE` | **write only** — set only when the check's `run` references it: absolute path to a temp file holding the proposed content, placed beside `$HECTOR_FILE` with the same extension. Auto-cleaned after the check. Unset on `pre-commit`. |
| stdin | The proposed post-edit content. |

The adapter only shells out to the `hector` binary. It doesn't reimplement any policy logic.

## The exit-code contract

Adapters translate [`hector check`'s exit codes](../operating/running-checks.md) into agent actions:

| `hector` exit | Adapter action |
|---------------|----------------|
| `0` (pass) | Allow the edit. |
| `2` (block) | Reject the edit; the agent retries. |
| `1` (config error) | **Fail-open** — log and allow. An unrelated problem, like a broken config, shouldn't block the agent's work. |
| `3` (internal error) | **Fail-open by default** — log and allow. Set `HECTOR_FAIL_CLOSED_ON_INTERNAL=1` to make internal errors block where the host lifecycle can still block. |

The fail-open default on internal errors is deliberate: a rule that *couldn't run* is not a rule that *found a problem*. To make internal errors blocking instead — for strict CI-style enforcement — set `HECTOR_FAIL_CLOSED_ON_INTERNAL=1`. See [Running checks](../operating/running-checks.md).

## Requirements

Every adapter needs:

- the `hector` binary on `PATH`,
- a `.hector.yml` in the project root,
- a trusted config (`hector trust` — or let `hector init` scaffold and trust one).

If hooks aren't firing for any agent, run [`hector doctor`](../operating/diagnostics.md) — it reports one adapter row per agent (detected / installed / registered / intact), so you can see exactly which wiring is missing.

## Managing policy from inside the agent

Adapters that support skills ship three for managing policy without leaving the session:

- **`hector-config`** is the authoring guide: the `{files, run}` check schema, the
  exit-code contract, and the common patterns, with a fixture-test loop. `hector
  init` installs it as a real skill into every detected agent, and `hector schema`
  prints it on demand.
- **`/hector-init`** scaffolds a `.hector.yml` from your project's stack.
- **`/hector-review`** reads your telemetry log and reports which checks are noisy,
  which never fire, and which look dead.

`hector init` installs `hector-config` for every agent it wires (all four support
the Agent Skills spec). `/hector-init` and `/hector-review` ship with the Claude
Code plugin today; other agents gain them as their needs settle.

## See also

- [Claude Code adapter](claude-code.md)
- [OpenCode adapter](opencode.md)
