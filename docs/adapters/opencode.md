# OpenCode adapter

The OpenCode adapter runs your Hector gates every time OpenCode edits or writes a file. When an edit breaks a gate, the adapter cancels the tool call, OpenCode hands the agent the verdict, and the agent rewrites the change to comply. The gate runs on every edit without you calling `hector check` by hand.

The adapter ships in this repo at `adapters/opencode/` as a TypeScript plugin.

## Install

With the `hector` binary on your `PATH`, one command wires the plugin and scaffolds a trusted config:

```bash
hector init --harness opencode
```

This writes the adapter to `<project>/.opencode/plugins/hector.ts` with a `.hector-adapter.json` sidecar (per-file sha256 + version) alongside it, then scaffolds and trusts a starter `.hector.yml`. OpenCode plugins are **project-scoped** — there is no global plugin directory, so `--global` has no effect here. Re-runs are idempotent (unchanged → "already present", changed artifact → "updated").

OpenCode ships Bun, so there is no separate runtime to install. Restart OpenCode so it discovers the plugin, then verify the wiring:

```bash
hector doctor
```

To remove it:

```bash
hector init --uninstall --harness opencode
```

This deletes the materialized plugin and its sidecar from `.opencode/plugins/`. Your `.hector.yml` and trust store are untouched.

If you wrote `.hector.yml` by hand instead of letting `hector init` scaffold it, trust it before checks will run:

```bash
hector trust
```

Hector runs the commands in your config, so it refuses to run one it hasn't seen. `hector trust` records the config in the trust store; any later edit invalidates it and you re-sign. See [The trust store](../security/trust.md).

## Watch it block an edit

Suppose your `.hector.yml` bans `DEBUG` markers in TypeScript:

```yaml
# .hector.yml
gates:
  no-debug:
    files: "**/*.ts"
    run: "! grep -nH 'DEBUG' \"$HECTOR_FILE\" || exit 2"
```

Ask OpenCode to add a `DEBUG` marker to a `.ts` file. Before the `edit` tool writes, the adapter checks the proposed content, the `no-debug` gate exits `2`, and the adapter throws. OpenCode cancels the tool call and surfaces the verdict to the agent, which sees that it broke `no-debug` and rewrites without the marker. A clean edit lands normally and you see nothing.

## What runs, and when

Every adapter follows the [same lifecycle](README.md#what-adapters-do); OpenCode covers it with a single pre-edit tool hook:

**Before every edit.** When OpenCode's `edit` or `write` tool proposes a change, the adapter shadow-writes the proposed content, runs `hector check --file <path>`, then restores the pre-edit file before OpenCode executes the tool. A block throws, and OpenCode cancels the edit so the agent retries.

## Manual install

Use these only if the `hector` binary isn't available (for example, bootstrapping a fresh machine before you can build it) — otherwise prefer `hector init` above, which writes the same file and keeps a sidecar so `hector doctor` can verify it.

Symlink the plugin source into the project's plugin directory and restart OpenCode:

```bash
mkdir -p .opencode/plugins
ln -sf /path/to/hector/adapters/opencode/src/index.ts .opencode/plugins/hector.ts
```

To gate **every** project at once, symlink into OpenCode's global plugin directory instead. (`hector init` is project-scoped; this manual route is the only way to install globally.) Because the plugin no-ops where there is no `.hector.yml`, a global install only acts on projects you have set up:

```bash
mkdir -p ~/.config/opencode/plugins
ln -sf /path/to/hector/adapters/opencode/src/index.ts ~/.config/opencode/plugins/hector.ts
```

Once the package is published, you can add it to a project's `opencode.json` and let OpenCode install it via Bun on first load:

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "plugin": ["@dynamik-dev/hector-opencode"]
}
```

## What isn't gated yet

A few edits still fall outside the adapter. Cover them by running `hector check` in CI:

- **Multi-file `apply_patch` edits.** OpenCode's batch patch tool bundles several files into one call, and the adapter does not split it apart. Use the `edit` and `write` tools for changes you want gated.
- **Skills.** `hector init` installs the `hector-config` authoring skill into `.opencode/skills/hector-config/SKILL.md` today — the agent can author and fix gates immediately; `hector schema` prints the same guide on demand. `/hector-init` and `/hector-review` are Claude Code plugin skills and are not ported to OpenCode.

## When edits aren't being gated

If OpenCode edits a file and nothing happens, walk through these in order:

1. Confirm `hector --version` runs on your `PATH`.
2. Confirm `.hector.yml` exists in the project root.
3. Confirm the config is trusted (`hector init` does this; otherwise run `hector trust`).
4. Confirm OpenCode loaded the plugin. It logs plugin discovery at startup; look for `hector.ts` in the log.
5. Run `hector doctor` — its `opencode` adapter row shows whether the plugin is installed and intact.
6. Run the bundled test against your build to prove the wiring end to end:

   ```bash
   PATH="$(pwd)/target/release:${PATH}" \
     bun test adapters/opencode/tests/plugin.test.ts
   ```

   Every case should pass: a clean file is allowed, a dirty file is blocked, and non-edit tools are ignored.

## How it works

The plugin is a small TypeScript module that consumes the `@opencode-ai/plugin` types and registers one hook: `tool.execute.before` for pre-edit gating. It only shells out to the `hector` binary via Bun's `$` API and holds no policy logic of its own, so changing a gate never touches the plugin. It translates `hector check`'s exit codes into allow/reject per [the exit-code contract](README.md#the-exit-code-contract) - the one wrinkle is that the plugin *throws* to make OpenCode cancel the tool call, where the Claude Code hook exits `2`.

## How it differs from the Claude Code adapter

The two adapters share the same contract: shell out to `hector`, gate edits on exit `2`, fail open on internal errors. They differ in the host's mechanics.

| Aspect | Claude Code | OpenCode |
|--------|-------------|----------|
| Language | bash + `jq` | TypeScript on Bun |
| Reject an edit | `PostToolUse` exit `2` | `tool.execute.before` throw |
| Skills | `hector-config` (via `hector init`) + `/hector-init`, `/hector-review` (plugin) | `hector-config` (via `hector init`) |

## See also

- [Adapters overview](README.md) — the fail-open contract every adapter shares
- [Claude Code adapter](claude-code.md) — the sibling adapter
- [Running checks](../operating/running-checks.md) — the exit codes the adapter keys off
