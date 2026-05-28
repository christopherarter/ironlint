# OpenCode adapter

The OpenCode adapter runs your Hector rules every time OpenCode edits or writes a file. When an edit breaks a rule, the adapter rejects the tool result, OpenCode hands the agent the verdict, and the agent rewrites the change to comply. The gate runs on every edit without you calling `hector check` by hand.

The adapter ships in this repo at `adapters/opencode/` as a TypeScript plugin.

## Install the plugin

You need the `hector` binary on your `PATH`. OpenCode ships Bun, so there is no separate runtime to install. Build Hector and confirm it is reachable:

```bash
cargo build --release   # produces ./target/release/hector
hector --version
```

Then symlink the plugin into your project's plugin directory and restart OpenCode:

```bash
mkdir -p .opencode/plugins
ln -sf /path/to/hector/adapters/opencode/src/index.ts .opencode/plugins/hector.ts
```

OpenCode discovers plugins in `.opencode/plugins/` on startup and loads `hector.ts` automatically.

## Set up a project

The plugin no-ops silently in any project without a `.hector.yml`, so it is safe to leave installed everywhere. To start gating a project, scaffold a config and sign it:

```bash
hector init     # detects your stack and writes a starter .hector.yml
hector trust    # fingerprints the config so Hector will run it
```

Hector runs the scripts in your config, so it refuses to run one it hasn't seen. `hector trust` writes a fingerprint; any later edit invalidates it and you re-sign. See [The trust gate](../security/trust.md).

## Watch it block an edit

Suppose your `.hector.yml` bans `DEBUG` markers in source:

```yaml
rules:
  no-debug:
    description: "no DEBUG markers in source"
    engine: script
    scope: ["src/**/*"]
    severity: error
    script: "grep -nE 'DEBUG' {file} && exit 1 || exit 0"
```

Ask OpenCode to add a debug print to a file under `src/`. The moment the `edit` tool finishes writing, the adapter runs `hector check` against that file, the `no-debug` rule fires, and the adapter throws. OpenCode rejects the tool result and surfaces the verdict to the agent, which sees that it broke `no-debug` and rewrites without the marker. A clean edit lands normally and you see nothing.

## What runs, and when

Every adapter follows the [same three-step lifecycle](README.md#what-every-adapter-does); OpenCode covers it with two hooks:

**After every edit.** When OpenCode's `edit` or `write` tool finishes writing to disk, the adapter records the change into `.hector/session.json`, then runs `hector check --file <path>`. A block throws, and OpenCode rejects the edit so the agent retries.

**When the agent goes idle.** On `session.idle`, the adapter runs `hector check --session` to evaluate your `session`-engine rules across the whole turn. This check is advisory: `session.idle` fires after the agent's response is already out, so a violation cannot retract the turn. The adapter surfaces it so the agent fixes it on the next iteration. See [Checking a whole edit session](../writing-rules/whole-session-checks.md).

**When a session starts.** On `session.created`, the adapter clears any stale `.hector/session.json` left by a previous run that aborted mid-turn.

## Judging semantic rules

Rules on the `semantic` and `session` engines ask an LLM to judge a change. Under OpenCode, point `llm:` at an API-key-backed provider:

```yaml
llm:
  provider: anthropic        # or openrouter, ollama
  model: claude-sonnet-4-6
```

The adapter reads your key from the provider's environment variable (`ANTHROPIC_API_KEY` for Anthropic) at check time. The `claude-code-subagent` mode is specific to Claude Code and does not apply here. See [LLM providers](../configuring/llm-providers.md) and [Asking an LLM to judge a change](../writing-rules/asking-an-llm.md).

## Installing across every project

To gate every project at once, symlink the plugin into OpenCode's global plugin directory instead of a single repo:

```bash
mkdir -p ~/.config/opencode/plugins
ln -sf /path/to/hector/adapters/opencode/src/index.ts ~/.config/opencode/plugins/hector.ts
```

Because the plugin no-ops where there is no `.hector.yml`, a global install only acts on projects you have set up.

Once the package is published, you can add it to a project's `opencode.json` and let OpenCode install it via Bun on first load:

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "plugin": ["@dynamik-dev/hector-opencode"]
}
```

## What isn't gated yet

A few edits fall outside the adapter at 0.1d. Cover them by running `hector check` in CI:

- **Multi-file `apply_patch` edits.** OpenCode's batch patch tool bundles several files into one call, and the adapter does not split it apart. Use the `edit` and `write` tools for changes you want gated.
- **Skills.** The [three policy skills](README.md#managing-policy-from-inside-the-agent) (`/hector-init`, `/hector-author`, `/hector-review`) are not wired into OpenCode yet. The skill files in `adapters/claude-code/skills/` follow the Agent Skills spec and will work in OpenCode once its skill-discovery path settles. Until then, run the CLI directly: `hector init`, then edit `.hector.yml` by hand.

## When edits aren't being gated

If OpenCode edits a file and nothing happens, walk through these in order:

1. Confirm `hector --version` runs on your `PATH`.
2. Confirm `.hector.yml` exists in the project root.
3. Confirm the config is trusted by running `hector trust`.
4. Confirm OpenCode loaded the plugin. It logs plugin discovery at startup; look for `hector.ts` in the log.
5. Run the bundled test against your build to prove the wiring end to end:

   ```bash
   PATH="$(pwd)/target/release:${PATH}" \
     bun test adapters/opencode/tests/plugin.test.ts
   ```

   Every case should pass: a clean file is allowed, a dirty file is blocked, the session lifecycle fires, and non-edit tools are ignored.

## How it works

The plugin is a small TypeScript module that consumes the `@opencode-ai/plugin` types and registers two hooks — `tool.execute.after` (for `edit` / `write`) and `event` (for `session.idle` / `session.created`), as described above. It only shells out to the `hector` binary via Bun's `$` API and holds no policy logic of its own, so changing a rule never touches the plugin. It translates `hector check`'s exit codes into allow/reject per [the exit-code contract](README.md#the-exit-code-contract) — the one wrinkle is that the plugin *throws* to make OpenCode reject the tool result, where the Claude Code hook exits `2`.

## How it differs from the Claude Code adapter

The two adapters share the same contract: shell out to `hector`, gate edits on exit `2`, fail open on internal errors. They differ in the host's mechanics.

| Aspect | Claude Code | OpenCode |
|--------|-------------|----------|
| Language | bash + `jq` | TypeScript on Bun |
| Reject an edit | `PostToolUse` exit `2` | `tool.execute.after` throw |
| End-of-turn check | `Stop` hook, can still block | `session.idle`, advisory only |
| Subagent mode | supported (`claude-code-subagent`) | not applicable |
| Skills | three shipped | not ported yet |

The one behavioral divergence is the end-of-turn check: Claude Code's `Stop` hook can still block the response, while OpenCode's `session.idle` fires after the response is out and can only flag the issue for the next turn.

## See also

- [Adapters overview](README.md) — the fail-open contract every adapter shares
- [Claude Code adapter](claude-code.md) — the sibling adapter, including subagent mode
- [Running checks](../operating/running-checks.md) — the exit codes the adapter keys off
- [Checking a whole edit session](../writing-rules/whole-session-checks.md) — what the idle check evaluates
