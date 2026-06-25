# Hector — pi adapter

[pi](https://pi.dev) extension integration for Hector. Mirrors the OpenCode and
Claude Code adapters: it gates `write` / `edit` tool calls against your
project's `.hector.yml` policy **before they execute**. It is a static,
per-file pre-write gate — each tool call is checked on its own.

The extension is a pure translation layer between pi's lifecycle and the
`hector` binary — it contains no policy logic of its own.

| pi event | Action |
|----------|--------|
| `tool_call` (`write` / `edit`) | Compute the proposed content, run `hector check --file <path> --content - --format json`, and `return { block: true, reason }` on a block (exit 2) — where `reason` is the blocking gate's message, parsed from the JSON verdict's `blocks[].message`. The check runs against piped stdin — nothing is written to disk. |

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
hector init    # scaffold .hector.yml (auto-blesses it in the trust store)
hector trust   # bless the config in the out-of-repo trust store
```

Trust is **required**: `check` fails closed (exit 1) on a config that is missing
from, or no longer matches, the trust store at `$XDG_CONFIG_HOME/hector/trust.json`
(else `~/.config/hector/trust.json`). The adapter treats exit 1 as a config error
and **allows** the edit (fail-open), so an untrusted config leaves the gate
silently inert — re-run `hector trust` after every edit to `.hector.yml`.

## Exit-code contract

The extension honours the `hector` CLI exit-code contract
(`crates/hector-cli/src/commands/check.rs`):

| Exit | Behaviour |
|------|-----------|
| `0` (pass) | Allow. |
| `2` (block) | `return { block: true, reason }` — pi cancels the tool call. |
| `3` (internal error) | Fail-open (log + allow) by default; set `HECTOR_FAIL_CLOSED_ON_INTERNAL=1` to fail closed (block). |
| `1` / other (config error, incl. untrusted or modified config) | Log to stderr, allow. |

## Known gaps (v1)

- **`bash`-tool shell-out** (`cat > foo`, redirections) bypasses the gate — universal across all adapters; arbitrary commands are too brittle to parse.
- **`edit` fuzzy-match fallback** can't be faithfully simulated, so those edits skip the gate (fail-open on simulate-failure). Exact + unique `oldText` edits gate normally.
- **Gates that read the file from disk** (via `$HECTOR_FILE`) see the *pre-edit* content; only the proposed post-edit content piped on **stdin** reflects the pending change. `hector-disable` directives carried in that proposed content are honoured.
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
