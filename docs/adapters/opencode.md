# OpenCode adapter

The OpenCode adapter runs your IronLint checks every time OpenCode edits or writes a file. When an edit breaks a check, the adapter cancels the tool call, OpenCode hands the agent the verdict, and the agent rewrites the change to comply. The check runs on every edit without you calling `ironlint check` by hand.

The adapter ships in this repo at `adapters/opencode/` as a TypeScript plugin.

## Install

With the `ironlint` binary on your `PATH`, one command wires the plugin and scaffolds a trusted config:

```bash
ironlint init --harness opencode
```

This writes the adapter to `<project>/.opencode/plugins/ironlint.ts` with a `.ironlint-adapter.json` sidecar (per-file sha256 + version) alongside it, then scaffolds and trusts a starter `.ironlint.yml`. `ironlint init` installs the OpenCode adapter at project scope, so `--global` does not change that path. Re-runs are idempotent (unchanged → "already present", changed artifact → "updated").

OpenCode ships Bun, so there is no separate runtime to install. Restart OpenCode so it discovers the plugin, then verify the wiring:

```bash
ironlint doctor
```

To remove it:

```bash
ironlint init --uninstall --harness opencode
```

This deletes the materialized plugin and its sidecar from `.opencode/plugins/`. Your `.ironlint.yml` and trust store are untouched.

If you wrote `.ironlint.yml` by hand instead of letting `ironlint init` scaffold it, trust it before checks will run:

```bash
ironlint trust
```

IronLint runs the commands in your config, so it refuses to run one it hasn't seen. `ironlint trust` records the config in the trust store; any later edit invalidates it and you re-sign. See [The trust store](../security/trust.md).

## Watch it block an edit

Suppose your `.ironlint.yml` bans `DEBUG` markers in TypeScript:

```yaml
# .ironlint.yml
checks:
  no-debug:
    files: "**/*.ts"
    run: "! grep -n 'DEBUG'"  # proposed content arrives on stdin
```

Ask OpenCode to add a `DEBUG` marker to a `.ts` file. Before the `edit` tool writes, the adapter checks the proposed content, the `no-debug` check exits nonzero, and the adapter throws. OpenCode cancels the tool call and surfaces the verdict to the agent, which sees that it broke `no-debug` and rewrites without the marker. A clean edit lands normally and you see nothing.

## What runs, and when

Every adapter follows the [same lifecycle](README.md#what-adapters-do); OpenCode covers it with a single pre-edit tool hook:

**Before every edit.** When OpenCode's `edit` or `write` tool proposes a change, the adapter shadow-writes the proposed content, runs `ironlint check --file <path>`, then restores the pre-edit file before OpenCode executes the tool. A block throws, and OpenCode cancels the edit so the agent retries.

## Manual install

Use these only if the `ironlint` binary isn't available (for example, bootstrapping a fresh machine before you can build it) — otherwise prefer `ironlint init` above, which writes the same file and keeps a sidecar so `ironlint doctor` can verify it.

Symlink the plugin source into the project's plugin directory and restart OpenCode:

```bash
mkdir -p .opencode/plugins
ln -sf /path/to/ironlint/adapters/opencode/src/index.ts .opencode/plugins/ironlint.ts
```

To cover **every** project at once, symlink into OpenCode's global plugin directory instead. (`ironlint init` is project-scoped; this manual route is the only way to install globally.) Because the plugin no-ops where there is no `.ironlint.yml`, a global install only acts on projects you have set up:

```bash
mkdir -p ~/.config/opencode/plugins
ln -sf /path/to/ironlint/adapters/opencode/src/index.ts ~/.config/opencode/plugins/ironlint.ts
```

## What isn't gated yet

A few edits still fall outside the adapter. Cover them by running `ironlint check` in CI:

- **Multi-file `apply_patch` edits.** OpenCode's batch patch tool bundles several files into one call, and the adapter does not split it apart. Use the `edit` and `write` tools for changes you want gated.
- **Skills.** `ironlint init` installs the `ironlint-config` authoring skill into `.opencode/skills/ironlint-config/SKILL.md` today — the agent can author and fix checks immediately; `ironlint schema` prints the same guide on demand. `/ironlint-init` and `/ironlint-review` are Claude Code plugin skills and are not ported to OpenCode.

## When edits aren't being gated

If OpenCode edits a file and nothing happens, walk through these in order:

1. Confirm `ironlint --version` runs on your `PATH`.
2. Confirm `.ironlint.yml` exists in the project root.
3. Confirm the config is trusted (`ironlint init` does this; otherwise run `ironlint trust`).
4. Confirm OpenCode loaded the plugin. It logs plugin discovery at startup; look for `ironlint.ts` in the log.
5. Run `ironlint doctor` — its `opencode` adapter row shows whether the plugin is installed and intact.
6. Run the bundled test against your build to prove the wiring end to end:

   ```bash
   PATH="$(pwd)/target/release:${PATH}" \
     bun test adapters/opencode/tests/plugin.test.ts
   ```

   Every case should pass: a clean file is allowed, a dirty file is blocked, and non-edit tools are ignored.

## How it works

The plugin is a small TypeScript module that consumes the `@opencode-ai/plugin` types and registers one hook: `tool.execute.before` for pre-edit gating. It only shells out to the `ironlint` binary via Bun's `$` API and holds no policy logic of its own, so changing a check never touches the plugin. It translates `ironlint check`'s exit codes into allow/reject per [the exit-code contract](README.md#the-exit-code-contract) - the one wrinkle is that the plugin *throws* to make OpenCode cancel the tool call, where the Claude Code hook exits `2`.

## How it differs from the Claude Code adapter

The two adapters share the same contract: shell out to `ironlint`, reject edits on exit `2` (block), fail open on internal errors. They differ in the host's mechanics.

| Aspect | Claude Code | OpenCode |
|--------|-------------|----------|
| Language | bash + `jq` | TypeScript on Bun |
| Reject an edit | `PreToolUse` exit `2` | `tool.execute.before` throw |
| Skills | `ironlint-config` (via `ironlint init`) + `/ironlint-init`, `/ironlint-review` (plugin) | `ironlint-config` (via `ironlint init`) |

## See also

- [Adapters overview](README.md) — the fail-open contract every adapter shares
- [Claude Code adapter](claude-code.md) — the sibling adapter
- [Running checks](../operating/running-checks.md) — the exit codes the adapter keys off
