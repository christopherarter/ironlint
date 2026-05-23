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
```

In this mode, the hook collects `engine: semantic` and `engine: session` rules into a `DeferredVerdict` payload and wraps it in Claude Code's `hookSpecificOutput.additionalContext` envelope (preamble: `AGENTIC LINT SEMANTIC EVALUATION REQUIRED:`). The next turn, the `hector` skill activates by description match, dispatches the `hector-evaluator` subagent (or inline-judges single-rule short-diff payloads), applies error-severity fixes via `Edit`, and calls `hector record-verdict` so the rule shows up in `hector coverage` telemetry.

Subagent-token billing rolls up under the parent session's subscription — no `ANTHROPIC_API_KEY` required.

**`model:` is optional under `claude-code-subagent`.** The subagent uses whatever model the Claude Code session is running, so `llm.model:` has no effect for this provider and can be omitted. If you set it anyway, hector emits a one-time stderr warning per process noting that the field is ignored. (Pre-R2 / 0.1 users typed `model: ignored` literally to satisfy the parser — that's no longer needed.)

#### Optional: pin the evaluator subagent's model

Add `llm.evaluator_model:` to request a specific model for the in-session `hector-evaluator` subagent — handy for keeping policy checks cheap (`haiku`) or strict (`opus`):

```yaml
llm:
  provider: claude-code-subagent
  evaluator_model: haiku    # propagated to the skill as a dispatch hint
```

The value is free-form; Claude Code validates the model id at dispatch time. The hector skill reads `payload.evaluator_model` from the `DeferredVerdict` envelope and surfaces the requested model in its reply. Today, Claude Code's subagent dispatch does not accept a per-call model override — the subagent's `model:` is fixed by the frontmatter in `adapters/claude-code/agents/hector-evaluator.md`. So the skill currently uses `evaluator_model` as an advisory: it notes the requested model and points you at the installed plugin copy of the subagent file to edit. If Claude Code adds inline dispatch overrides, the skill will switch to passing the value directly.

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
