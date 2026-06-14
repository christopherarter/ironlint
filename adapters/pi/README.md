# Hector — pi adapter

[pi](https://pi.dev) extension integration for Hector. Mirrors the OpenCode and
Claude Code adapters: it gates `write` / `edit` tool calls against your
project's `.hector.yml` policy **before they execute**. It is a static,
per-file pre-write gate — each tool call is checked on its own.

The extension is a pure translation layer between pi's lifecycle and the
`hector` binary — it contains no rule logic.

| pi event | Action |
|----------|--------|
| `tool_call` (`write` / `edit`) | Compute the proposed content, run `hector check --file <path> --content -`, and `return { block: true, reason }` on a policy violation (exit 2). The check runs against piped stdin — nothing is written to disk. |

## Requirements

- The `hector` binary on `PATH` (`cargo install hector` or a release binary), ≥ 0.1.
- Node ≥ 22.6 (pi's runtime; also required for the bundled `node:test` suite).

## Install

The extension silently no-ops in any project without a `.hector.yml`, so a
global install is safe.

### Local development

Copy or symlink the source into a pi extensions directory:

```bash
# project-scoped
mkdir -p .pi/extensions
ln -sf "$(pwd)/../hector/adapters/pi/src/index.ts" .pi/extensions/hector.ts

# or global
mkdir -p ~/.pi/agent/extensions
ln -sf "/abs/path/to/hector/adapters/pi/src/index.ts" ~/.pi/agent/extensions/hector.ts
```

Or reference an absolute path in pi `settings.json`:

```json
{ "extensions": ["/abs/path/to/hector/adapters/pi/src/index.ts"] }
```

Ad-hoc load for one session: `pi -e ./adapters/pi/src/index.ts`. Hot-reload
with `/reload`.

### npm (once published)

`@dynamik-dev/hector-pi` ships a `"pi": { "extensions": ["./src/index.ts"] }`
field, so pi discovers it automatically once the package is installed.

## Initialise the project

```bash
hector init    # scaffold .hector.yml
hector trust   # fingerprint the config
```

## Exit-code contract

The extension honours the `hector` CLI exit-code contract
(`crates/hector-cli/src/commands/check.rs`):

| Exit | Behaviour |
|------|-----------|
| `0` (pass / warn) | Allow. |
| `2` (block) | `return { block: true, reason }` — pi cancels the tool call. |
| `3` (engine internal error) | Fail-open (log + allow) by default; set `HECTOR_FAIL_CLOSED_ON_INTERNAL=1` to fail closed (block). |
| `1` / other (config error) | Log to stderr, allow. |

## Known gaps (v1)

- **`bash`-tool shell-out** (`cat > foo`, redirections) bypasses the gate — universal across all adapters; arbitrary commands are too brittle to parse.
- **`edit` fuzzy-match fallback** can't be faithfully simulated, so those edits skip the gate (fail-open on simulate-failure). Exact + unique `oldText` edits gate normally.
- **`engine: script` rules** read the pre-edit on-disk file under `--content -`. AST and `hector-disable` rules gate correctly against the proposed pre-write content.
- **pi subagents** are not specially handled (deferred).
- **No cross-edit checks.** The gate evaluates each `write` / `edit` in isolation; it does not aggregate edits across a turn.

## Diagnostic

If the gate isn't firing:

1. `hector --version` runs on `PATH`.
2. `.hector.yml` is present in the project root.
3. `.hector.yml` is trusted: `hector trust`.
4. pi loaded the extension (check pi's extension discovery logs / `/reload`).
5. Run the bundled suite against your install:

   ```bash
   PATH="$(pwd)/target/release:${PATH}" \
     node --experimental-strip-types --test adapters/pi/test/*.test.ts
   ```
