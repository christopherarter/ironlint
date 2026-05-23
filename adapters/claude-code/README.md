# Hector — Claude Code adapter

`/plugin install` integration for Claude Code. Provides:

- `PostToolUse` hook: runs `hector check` on every `Edit` or `Write` tool call.
- `Stop` hook: runs `hector check --session` over the accumulated changeset.
- `SessionStart` hook: clears stale session state from a previous run.
- Skills: `/hector-init`, `/hector-author`, `/hector-review`.

## Install

1. Install the `hector` binary (`cargo install hector`, or use a release binary).
2. Add this plugin via your Claude Code plugin manager.
3. Run `/hector-init` in a project to scaffold `.hector.yml`.
4. Review the config and run `hector trust` to fingerprint it.
5. Edit any file — the PostToolUse hook will gate edits against the rules.

## Requirements

- `hector` binary on PATH.
- `jq` on PATH (parse PostToolUse event payloads).
- `bash` (the hook script is bash).

## Modes

The adapter supports two semantic-evaluation paths. Pick one based on which Claude Code account type you're using.

### Direct-API mode (default)

Set `llm:` to any of the API-key-backed providers:

```yaml
llm:
  provider: anthropic       # or openrouter, ollama
  model: claude-3-5-sonnet-20241022
```

The PostToolUse hook calls the LLM directly. Requires `ANTHROPIC_API_KEY` (or the matching provider env var) in the user's environment. Best fit for API users and CI.

### Subagent mode (Claude Code subscription)

Set `llm.provider` to `claude-code-subagent`:

```yaml
llm:
  provider: claude-code-subagent
  model: subagent           # placeholder — the LLM is never dispatched
```

In this mode, the hook collects `engine: semantic` and `engine: session` rules into a `DeferredVerdict` payload and wraps it in Claude Code's `hookSpecificOutput.additionalContext` envelope (preamble: `AGENTIC LINT SEMANTIC EVALUATION REQUIRED:`). The next turn, the `hector` skill activates by description match, dispatches the `hector-evaluator` subagent (or inline-judges single-rule short-diff payloads), applies error-severity fixes via `Edit`, and calls `hector record-verdict` so the rule shows up in `hector coverage` telemetry.

Subagent-token billing rolls up under the parent session's subscription — no `ANTHROPIC_API_KEY` required. The `model:` field is still required by the config parser but is never read in subagent mode; any non-empty string works.

Deterministic rules (script + AST) run identically in both modes. Only the semantic / session paths differ.

## How the hooks resolve

`hooks/hooks.json` dispatches each event to `"${CLAUDE_PLUGIN_ROOT}/hooks/hook.sh"`.
`CLAUDE_PLUGIN_ROOT` is set by Claude Code at hook-fire time and points to the
plugin's installed directory (wherever the plugin manager unpacked this adapter).
You do **not** set it yourself.

If a hook fails with `hook.sh: No such file or directory`, the plugin is not
installed where Claude Code expects. Reinstall with `/plugin install` or, for
local development, symlink this directory under your plugins root. See
[`docs/adapters/claude-code.md`](../../docs/adapters/claude-code.md) for full
install paths and diagnostic steps.
