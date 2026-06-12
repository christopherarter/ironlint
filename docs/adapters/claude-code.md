# Claude Code adapter

The Claude Code adapter runs your Hector rules every time Claude edits a file. When an edit breaks a rule, Claude Code rejects it on the spot, hands Claude the verdict, and Claude rewrites the change to comply. You stop having to remember to run `hector check` yourself; the gate is always on.

The adapter ships in this repo at `adapters/claude-code/`.

## Install the plugin

You need the `hector` binary and `jq` on your `PATH` first. Build Hector and check both are reachable:

```bash
cargo build --release   # produces ./target/release/hector
hector --version
jq --version
```

Then link the adapter into Claude Code's plugin directory and restart so it loads:

```bash
ln -sf "$(pwd)/adapters/claude-code" ~/.claude/plugins/data/hector
```

Once Hector is published to the plugin marketplace you can skip the symlink and run `/plugin install hector` instead.

## Set up a project

The adapter stays silent in any project that has no `.hector.yml`, so installing it globally is safe. To start gating a project, scaffold a config and sign it.

Run `/hector-init` in the project. Claude detects your stack (Rust, Node, Python) and writes a starter `.hector.yml`. Review the rules it generated, then trust the file:

```bash
hector trust
```

Hector runs the scripts in your config, so it refuses to run one it hasn't seen. `hector trust` writes a fingerprint into the config; any later edit invalidates it and you re-sign. See [The trust gate](../security/trust.md) for why.

## Watch it block an edit

Here is the whole point of the adapter, end to end. Suppose your `.hector.yml` bans `DEBUG` markers in source:

```yaml
rules:
  no-debug:
    description: "no DEBUG markers in source"
    engine: script
    scope: ["src/**/*"]
    severity: error
    script: "grep -nE 'DEBUG' {file} && exit 1 || exit 0"
```

Ask Claude to add a debug print to a file under `src/`. The instant Claude writes the edit, the adapter runs `hector check` against that file, the `no-debug` rule fires, and Claude Code rejects the edit. Claude reads the returned block message — a plain-text summary naming the rule and the tool's own complaint — sees that it broke `no-debug`, and rewrites the change without the marker. The retry happens in the transcript while you watch; you never touched the keyboard.

A clean edit, one that breaks no rule, lands normally and you see nothing at all. That silence is the adapter working.

## What runs, and when

Every adapter follows the [same lifecycle](README.md#what-adapters-do); here is how Claude Code wires it:

**After every edit.** When Claude finishes an `Edit` or `Write`, the adapter records the change into `.hector/session.json`, then runs `hector check --file <path>`. A block rejects the edit and Claude retries. This is the gate you saw above.

**When Claude finishes its turn.** On `Stop`, the adapter runs `hector check --session` to evaluate your `session`-engine rules, the ones that reason across every edit in the turn rather than one file at a time. See [Checking a whole edit session](../writing-rules/whole-session-checks.md).

**When a session starts.** The adapter clears any stale `.hector/session.json` left behind by a previous run that aborted mid-turn, so a fresh session starts from a clean slate.

## Choosing how semantic rules get judged

Some rules ask an LLM to judge a change rather than grep for a pattern: the `semantic` and `session` engines. How Hector makes that LLM call depends on how you pay for Claude. Pick the mode that matches your setup in your config's `llm:` block.

### When you use a direct provider

This is the default. Point `llm:` at a provider Hector can call directly:

```yaml
llm:
  provider: anthropic        # or openrouter, ollama
  model: claude-sonnet-4-6
```

The adapter calls the model directly on each edit. API-backed providers read credentials from their environment variable (`ANTHROPIC_API_KEY` for Anthropic); local providers such as Ollama use their configured local endpoint. This is the right fit for API users, local model users, and CI.

### When you're on a Claude subscription

If you run Claude Code on a subscription and have no API key, set the provider to `claude-code-subagent`:

```yaml
llm:
  provider: claude-code-subagent
```

In this mode the adapter does not call any API. Instead it hands the pending semantic check back to Claude Code as added context. On the next turn the bundled `hector` skill picks that up, dispatches the `hector-evaluator` subagent to judge the change, applies any error-severity fixes, and records the verdict in your [telemetry log](../operating/telemetry.md) so the rule isn't counted as dead. The subagent's tokens bill under your session's subscription, so no API key is required.

`model:` has no effect under `claude-code-subagent`, since the subagent uses whatever model your Claude Code session is already running. You can leave it out. See [LLM providers](../configuring/llm-providers.md) for the full picture and [Asking an LLM to judge a change](../writing-rules/asking-an-llm.md) for writing the rules themselves.

## Author and review rules from inside Claude

The adapter ships the three standard policy skills — `/hector-init`, `/hector-author`, and `/hector-review`. See [Managing policy from inside the agent](README.md#managing-policy-from-inside-the-agent) for what each does.

## When edits aren't being gated

If Claude edits a file and nothing happens, walk through these in order:

1. Confirm the plugin landed where Claude Code expects it. The hook lives at `${CLAUDE_PLUGIN_ROOT}/hooks/hook.sh` and must be executable. If you see `hook.sh: No such file or directory`, reinstall or re-create the symlink above.
2. Confirm `hector --version` runs on your `PATH`.
3. Confirm `.hector.yml` exists in the project root.
4. Confirm the config is trusted by running `hector trust`.
5. Trace a single event end to end: `bash -x adapters/claude-code/hooks/hook.sh post-tool-use < event.json`.

For a one-shot health check, run [`hector doctor`](../operating/diagnostics.md). Its `adapter` check confirms the wiring without you tracing anything by hand.

## How it works

The adapter is one bash script that Claude Code calls on the three hook events above — `PostToolUse` (matching `Edit` \| `Write`), `Stop`, and `SessionStart`. It only ever shells out to the `hector` binary and holds no policy logic of its own, so changing a rule never means touching the adapter. It translates `hector check`'s exit codes into allow/reject per [the exit-code contract](README.md#the-exit-code-contract). The adapter gates edits and nothing else — it does not proxy Claude's `Read`, `Grep`, or `Glob` tools.

## See also

- [Adapters overview](README.md) — the fail-open contract every adapter shares
- [Running checks](../operating/running-checks.md) — the exit codes the adapter keys off
- [Checking a whole edit session](../writing-rules/whole-session-checks.md) — what the `Stop` hook evaluates
- [LLM providers](../configuring/llm-providers.md) — direct-API and subagent modes in full
