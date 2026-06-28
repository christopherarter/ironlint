# Hector — OpenCode adapter

OpenCode plugin integration for Hector. A static per-file gate that uses OpenCode's pre-edit hook to run your rules before each edit lands.

| OpenCode hook | Action |
|---------------|--------|
| `tool.execute.before` (`edit` / `write`) | Shadow-write the proposed content, run `hector check --file <path>`, restore the original file, and throw on block so OpenCode cancels the tool call. |

## Install

```bash
hector init --harness opencode
```

This writes the adapter plugin atomically to `<project>/.opencode/plugins/hector.ts`
(project-scoped; OpenCode does not expose a global plugin directory). A
`.hector-adapter.json` sidecar (per-file sha256 + version) is placed alongside
the artifact. Re-runs are idempotent (unchanged → "already present", changed
artifact → "updated").

Verify the install:

```bash
hector doctor
```

To remove the hook:

```bash
hector init --uninstall --harness opencode
```

This removes the materialized artifact and sidecar from `.opencode/plugins/`.
Your `.hector.yml` and trust store are untouched.

## Requirements

- The `hector` binary on PATH (`cargo install hector` or release binary).
- OpenCode (which ships Bun). No extra runtime install required.

## Manual fallback

Use these steps if the `hector` binary is not available (e.g., bootstrapping a
fresh machine before you can build):

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

The plugin is a small TypeScript module that consumes the `@opencode-ai/plugin` types. It registers one hook:

- **`tool.execute.before`** — fires before OpenCode's built-in `edit` / `write` tools write to disk. The plugin computes the proposed content, shadow-writes it to the target path, invokes `hector check --file <path>` via Bun's `$` shell API, then restores the original file. On exit code `2` (block), it throws an `Error` whose message is the JSON verdict, so OpenCode cancels the tool call before the edit lands.

The `hector` binary is the only authoritative source of rule logic. The plugin is purely a translation layer; rule changes never touch the plugin.

## Exit-code contract

The plugin honours the `hector` CLI exit-code contract from `commands/check.rs`:

| Exit | Plugin behaviour |
|------|------------------|
| `0` (pass or warn) | Allow. |
| `2` (block) | Throw — OpenCode cancels the tool call. |
| `3` (engine internal error) | Fail-open by default; set `HECTOR_FAIL_CLOSED_ON_INTERNAL=1` to fail closed while the hook can still block. |
| `1` / other (config error) | Log to stderr, allow. Config errors should not block the agent on unrelated work. |

## Known gaps at 0.1d

- **No `apply_patch` interception.** OpenCode's multi-file patch tool would need per-file extraction; large refactors via `apply_patch` are not gated. Use `edit` / `write` or run `hector check` manually in CI to cover them.
- **Partially ported skills.** `hector init` installs the `hector-config` authoring skill into `.opencode/skills/` today — gate authoring and the fixture-test loop are available in OpenCode. `/hector-init` and `/hector-review` remain Claude Code plugin-only skills and are not yet ported to other harnesses.

## Diagnostic

If hooks aren't firing:

1. Check `hector --version` runs on PATH.
2. Check `.hector.yml` is present in the project root.
3. Check `.hector.yml` is trusted: `hector trust`.
4. Confirm the plugin is loaded — OpenCode logs plugin discovery at startup.
5. Run `hector doctor` for a structured health report.
6. Run the bundled test against your install:

   ```bash
   PATH="$(pwd)/target/release:${PATH}" \
     bun test adapters/opencode/tests/plugin.test.ts
   ```
