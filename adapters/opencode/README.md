# IronLint — OpenCode adapter

OpenCode plugin integration for IronLint. A static per-file check that uses OpenCode's pre-edit hook to run checks before each edit lands.

| OpenCode hook | Action |
|---------------|--------|
| `tool.execute.before` (`edit` / `write`) | Shadow-write the proposed content, run `ironlint check --file <path>`, restore the original file, and throw on block so OpenCode cancels the tool call. |

## Install

```bash
ironlint init --harness opencode
```

This writes the adapter plugin atomically to `<project>/.opencode/plugins/ironlint.ts`
(project-scoped; OpenCode does not expose a global plugin directory). A
`.ironlint-adapter.json` sidecar (per-file sha256 + version) is placed alongside
the artifact. Re-runs are idempotent (unchanged → "already present", changed
artifact → "updated").

Verify the install:

```bash
ironlint doctor
```

To remove the hook:

```bash
ironlint init --uninstall --harness opencode
```

This removes the materialized artifact and sidecar from `.opencode/plugins/`.
Your `.ironlint.yml` and trust store are untouched.

## Requirements

- The `ironlint` binary on PATH (`cargo install --git https://github.com/christopherarter/ironlint ironlint-cli` or release binary).
- OpenCode (which ships Bun). No extra runtime install required.

## Manual fallback

Use these steps if the `ironlint` binary is not available (e.g., bootstrapping a
fresh machine before you can build):

### Local development

Symlink the plugin source into your project's plugin directory:

```bash
mkdir -p .opencode/plugins
ln -sf "$(pwd)/../ironlint/adapters/opencode/src/index.ts" .opencode/plugins/ironlint.ts
```

Or copy the file:

```bash
cp /path/to/ironlint/adapters/opencode/src/index.ts .opencode/plugins/ironlint.ts
```

Restart OpenCode. The plugin will be picked up automatically.

### npm (once published)

```jsonc
// opencode.json
{
  "$schema": "https://opencode.ai/config.json",
  "plugin": ["@christopherarter/ironlint-opencode"]
}
```

OpenCode installs the package via Bun on first load.

## Initialise the project

Run `ironlint init` to scaffold `.ironlint.yml`, review the checks, then:

```bash
ironlint trust
```

This fingerprints the config. The plugin no-ops silently in projects without `.ironlint.yml`, so installing it globally is safe.

## How it works

The plugin is a small TypeScript module that consumes the `@opencode-ai/plugin` types. It registers one hook:

- **`tool.execute.before`** — fires before OpenCode's built-in `edit` / `write` tools write to disk. The plugin computes the proposed content, shadow-writes it to the target path, invokes `ironlint check --file <path>` via Bun's `$` shell API, then restores the original file. On exit code `2` (block), it throws an `Error` whose message is the JSON verdict, so OpenCode cancels the tool call before the edit lands.

The `ironlint` binary is the only authoritative source of check logic. The plugin is purely a translation layer; check changes never touch the plugin.

## Exit-code contract

The plugin honours the `ironlint` CLI exit-code contract from `commands/check.rs`:

| Exit | Plugin behaviour |
|------|------------------|
| `0` (pass or warn) | Allow. |
| `2` (block) | Throw — OpenCode cancels the tool call. |
| `3` (internal error) | Fail-open by default; set `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1` to fail closed while the hook can still block. |
| `1` / other (config error) | Log to stderr, allow. Config errors should not block the agent on unrelated work. |

## Bash gate

In addition to file edits, this adapter gates `bash` (the agent's shell tool —
`tool:"bash"`, command in `args.command`). Commands that would let the agent
free itself — `ironlint trust`, or a Bash write to `.ironlint.yml` /
`.ironlint/gates/` — are denied (throw, mirroring the exit-2 edit path).
Ordinary commands are not slowed: a substring pre-filter skips the decision
entirely for commands that never mention `ironlint` or `.ironlint`. The deny
decision is shared across every adapter via `ironlint gate-bash`. The branch
runs before the config-existence check, so it fires even in a project with no
`.ironlint.yml`. See
`docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`
for the threat model and the documented known gap (variable-substitution
indirection).

## Known gaps at 0.1d

- **No `apply_patch` interception.** OpenCode's multi-file patch tool would need per-file extraction; large refactors via `apply_patch` are not gated. Use `edit` / `write` or run `ironlint check` manually in CI to cover them.
- **Partially ported skills.** `ironlint init` installs the `ironlint-config` authoring skill into `.opencode/skills/` today — check authoring and the fixture-test loop are available in OpenCode. `/ironlint-init` and `/ironlint-review` remain Claude Code plugin-only skills and are not yet ported to other harnesses.

## Diagnostic

If hooks aren't firing:

1. Check `ironlint --version` runs on PATH.
2. Check `.ironlint.yml` is present in the project root.
3. Check `.ironlint.yml` is trusted: `ironlint trust`.
4. Confirm the plugin is loaded — OpenCode logs plugin discovery at startup.
5. Run `ironlint doctor` for a structured health report.
6. Run the bundled test against your install:

   ```bash
   PATH="$(pwd)/target/release:${PATH}" \
     bun test adapters/opencode/tests/plugin.test.ts
   ```
