# Hector — OpenCode adapter

OpenCode plugin integration for Hector. Mirrors the Claude Code adapter behaviour.

| OpenCode hook | Action |
|---------------|--------|
| `tool.execute.after` (`edit` / `write`) | Record the edit in `.hector/session.json`, then run `hector check --file <path>`. Throw on block → OpenCode rejects the tool result and the agent retries. |
| `event` → `session.idle` | Run `hector check --session` over the accumulated changeset. Throw on block → surfaced to the user. |
| `event` → `session.created` | Clear stale `.hector/session.json` from a prior aborted run. |

## Install

You need:
- The `hector` binary on PATH (`cargo install hector` or release binary).
- OpenCode (which ships Bun). No extra runtime install required.

### Local development

Symlink the plugin source into your project's plugin directory:

```bash
mkdir -p .opencode/plugins
ln -sf "$(pwd)/../hector/adapters/opencode/src/index.ts" .opencode/plugins/hector.ts
```

Or copy the file:

```bash
cp /path/to/hector/adapters/opencode/src/index.ts .opencode/plugins/hector.ts
```

Restart OpenCode. The plugin will be picked up automatically.

### npm (once published)

```jsonc
// opencode.json
{
  "$schema": "https://opencode.ai/config.json",
  "plugin": ["@dynamik-dev/hector-opencode"]
}
```

OpenCode installs the package via Bun on first load.

## Initialise the project

Run `hector init` to scaffold `.hector.yml`, review the rules, then:

```bash
hector trust
```

This fingerprints the config. The plugin no-ops silently in projects without `.hector.yml`, so installing it globally is safe.

## How it works

The plugin is a small TypeScript module that consumes the `@opencode-ai/plugin` types. It registers exactly two hooks:

- **`tool.execute.after`** — fires after OpenCode's built-in `edit` / `write` tools finish writing to disk. The plugin invokes `hector check --file <path>` via Bun's `$` shell API. On exit code `2` (block), it throws an `Error` whose message is the JSON verdict — OpenCode surfaces this to the agent, which sees the rejection and retries.
- **`event`** — filtered on `event.type`. On `session.created`, the plugin clears `.hector/session.json`. On `session.idle`, it runs `hector check --session` to evaluate `session`-engine rules over the accumulated changeset.

The `hector` binary is the only authoritative source of rule logic. The plugin is purely a translation layer; rule changes never touch the plugin.

## Exit-code contract

The plugin honours the `hector` CLI exit-code contract from `commands/check.rs`:

| Exit | Plugin behaviour |
|------|------------------|
| `0` (pass or warn) | Allow. |
| `2` (block) | Throw — OpenCode rejects the tool result. |
| `1` or other (internal) | Log to stderr, allow. Internal hector errors should not block the agent on unrelated work. |

## Known gaps at 0.1d

- **No `apply_patch` interception.** OpenCode's multi-file patch tool would need per-file extraction; large refactors via `apply_patch` are not gated. Use `edit` / `write` or run `hector check` manually in CI to cover them.
- **No skills.** The Claude Code adapter ships `/hector-init`, `/hector-author`, `/hector-review`. Those SKILL.md files live in `adapters/claude-code/skills/` and are written against the Anthropic Skills spec — they'll work in OpenCode once we settle on a shared skills directory or a sidecar install (`malhashemi/opencode-skills`).
- **`session.idle` is advisory.** OpenCode's `session.idle` fires after the agent's response — the plugin cannot retroactively block the turn. Violations surface via `console.error` and a thrown error so the user sees what to fix next iteration.

## Requirements

- `hector` ≥ 0.1 on PATH.
- OpenCode (any version that supports the plugin Hooks interface with `tool.execute.after` and `event`).

## Diagnostic

If hooks aren't firing:

1. Check `hector --version` runs on PATH.
2. Check `.hector.yml` is present in the project root.
3. Check `.hector.yml` is trusted: `hector trust`.
4. Confirm the plugin is loaded — OpenCode logs plugin discovery at startup.
5. Run the bundled test against your install:

   ```bash
   PATH="$(pwd)/target/release:${PATH}" \
     bun test adapters/opencode/tests/plugin.test.ts
   ```
