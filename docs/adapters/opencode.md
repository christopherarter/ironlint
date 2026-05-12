# OpenCode adapter

The OpenCode adapter ships in this repo at `adapters/opencode/`. It exposes hector to OpenCode as a TypeScript plugin loaded from `.opencode/plugins/` or via npm.

## What it does

| OpenCode hook | Action |
|---------------|--------|
| `tool.execute.after` (`edit` / `write`) | Record the edit in `.hector/session.json`, then run `hector check --file <path>`. Throw on block — OpenCode rejects the tool result and the agent retries. |
| `event` → `session.created` | Clear stale `.hector/session.json` from a prior aborted run. |
| `event` → `session.idle` | Run `hector check --session` to evaluate session-engine rules. Throw on block — surfaced to the user. |

## What it does NOT do

- No `apply_patch` gating. OpenCode's multi-file patch tool uses `output.args.patchText` with embedded `*** Add File: …` markers — extracting per-file content is out of scope at 0.1d. Use `edit` / `write` for gated changes; run `hector check` in CI to cover anything else.
- No `tool.execute.before` interception. Hector engines read files from disk; running `before` would require synthesising the post-edit content in memory. `tool.execute.after` matches the Claude Code `PostToolUse` semantics — the file is on disk, hector checks it, the throw surfaces a rejection.
- No skills (yet). See [Skills](#skills) below.

## Install paths

### Local development

Symlink the plugin source into your project:

```bash
cd /path/to/your/project
mkdir -p .opencode/plugins
ln -sf /path/to/hector/adapters/opencode/src/index.ts .opencode/plugins/hector.ts
```

Restart OpenCode. The plugin loads from `.opencode/plugins/` automatically.

### Global

Same idea, into the OpenCode global plugin directory:

```bash
mkdir -p ~/.config/opencode/plugins
ln -sf /path/to/hector/adapters/opencode/src/index.ts ~/.config/opencode/plugins/hector.ts
```

### npm (once published)

Add the plugin to your `opencode.json`:

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "plugin": ["@dynamik-dev/hector-opencode"]
}
```

OpenCode installs the npm package via Bun on first load.

## Requirements

- `hector` binary on PATH (`cargo install hector` or release binary).
- OpenCode (any version supporting the `Hooks` interface with `tool.execute.after` and `event`).
- Bun (shipped with OpenCode — no separate install).

## Skills

The Claude Code adapter ships three skills (`/hector-init`, `/hector-author`, `/hector-review`). They are not ported to OpenCode at 0.1d.

Workarounds:
- The SKILL.md files in `adapters/claude-code/skills/` follow Anthropic's Agent Skills Specification. Once OpenCode's skill-discovery path stabilises (or once `malhashemi/opencode-skills` is added), these files will work directly without changes.
- Until then, you can manually `cat` the SKILL.md content into the OpenCode prompt when you want that workflow.

## Diagnostic

If hooks aren't firing:

1. Check `hector --version` runs on PATH.
2. Check `.hector.yml` is present in the project root.
3. Check the config is trusted: `hector trust`.
4. Confirm OpenCode is loading the plugin — OpenCode logs plugin discovery at startup; look for `hector.ts` in the log.
5. Run the bundled test against your install:

   ```bash
   cargo build --release
   PATH="$(pwd)/target/release:${PATH}" \
     bun test adapters/opencode/tests/plugin.test.ts
   ```

   All cases should pass: clean file allowed, dirty file blocked, session lifecycle, non-edit tools ignored.

## Exit-code mapping

The plugin honours the `hector` CLI exit-code contract (`commands/check.rs`):

| `hector` exit | Plugin behaviour |
|---------------|------------------|
| `0` (pass / warn) | Allow. |
| `2` (block) | Throw — OpenCode rejects the tool result. |
| `1` or other (internal) | Log to stderr, allow. Internal hector errors must not block the agent on unrelated work. |

## Comparison to the Claude Code adapter

| Aspect | Claude Code | OpenCode |
|--------|-------------|----------|
| Language | bash + `jq` | TypeScript |
| Tool gating | `PostToolUse` (`Edit\|Write`) → exit 2 | `tool.execute.after` (`edit`/`write`) → throw |
| Session start | `SessionStart` hook | `event` filter (`session.created`) |
| Session end | `Stop` hook (blocks the response) | `event` filter (`session.idle`, advisory only) |
| Hector binary | Shells out via `bash` | Shells out via Bun `$` |
| Install | `/plugin install` | File drop into `.opencode/plugins/` or npm |
| Skills | Three SKILL.md skills | Not ported at 0.1d |

The semantic contract is identical — the only behavioural divergence is that `session.idle` is advisory in OpenCode (the agent's response has already been emitted by the time the event fires), whereas Claude Code's `Stop` hook can still block.
