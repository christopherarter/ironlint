# IronLint — pi adapter

[pi](https://pi.dev) extension integration for IronLint. Mirrors the OpenCode and
Claude Code adapters: it runs checks on `write` / `edit` tool calls against your
project's `.ironlint.yml` policy **before they execute**. It is a static,
per-file pre-write check — each tool call is evaluated on its own.

The extension is a pure translation layer between pi's lifecycle and the
`ironlint` binary — it contains no policy logic of its own.

| pi event | Action |
|----------|--------|
| `tool_call` (`write` / `edit`) | Compute the proposed content, run `ironlint check --file <path> --content - --format json`, and `return { block: true, reason }` on a block (exit 2) — where `reason` is the blocking check's message, parsed from the JSON verdict's `blocks[].message`. The check runs against piped stdin — nothing is written to disk. |

## Install

```bash
ironlint init --harness pi
```

This writes the adapter plugin atomically to `<project>/.pi/extensions/ironlint.ts`
(default) or `~/.pi/agent/extensions/ironlint.ts` with `--global`. A
`.ironlint-adapter.json` sidecar (per-file sha256 + version) is placed alongside
the artifact. A backup of any prior settings is saved as `<settings>.bak` on the
first write; re-runs are idempotent (unchanged → "already present", changed
artifact → "updated").

Verify the install:

```bash
ironlint doctor
```

To remove the hook:

```bash
ironlint init --uninstall --harness pi
```

This removes the materialized artifact and sidecar from the extensions directory.
Your `.ironlint.yml` and trust store are untouched.

## Requirements

- The `ironlint` binary on `PATH` (`cargo install --git https://github.com/ironlint/ironlint ironlint-cli` or a release binary).
- Node ≥ 22.6 (pi's runtime; also required for the bundled `node:test` suite).

## Manual fallback

Use these steps if the `ironlint` binary is not available (e.g., bootstrapping a
fresh machine before you can build):

### Local development

Copy or symlink the source into a pi extensions directory:

```bash
# project-scoped
mkdir -p .pi/extensions
ln -sf "$(pwd)/../ironlint/adapters/pi/src/index.ts" .pi/extensions/ironlint.ts

# or global
mkdir -p ~/.pi/agent/extensions
ln -sf "/abs/path/to/ironlint/adapters/pi/src/index.ts" ~/.pi/agent/extensions/ironlint.ts
```

Or reference an absolute path in pi `settings.json`:

```json
{ "extensions": ["/abs/path/to/ironlint/adapters/pi/src/index.ts"] }
```

Ad-hoc load for one session: `pi -e ./adapters/pi/src/index.ts`. Hot-reload
with `/reload`.

### npm package

An npm package is not published yet. Until a release announces an installable
package and its final name, use `ironlint init --harness pi` or the local
development setup above.

## Initialise the project

```bash
ironlint init    # scaffold and bless a new .ironlint.yml
```

Trust is required: `check` exits `4` when a config is missing from, or no longer
matches, the trust store at `$XDG_CONFIG_HOME/ironlint/trust.json` (or
`~/.config/ironlint/trust.json`). The adapter treats that exit as fail-closed,
so an untrusted policy blocks the edit and tells you to run `ironlint trust`.
Re-run `ironlint trust` after every edit to `.ironlint.yml`.

## Exit-code contract

The extension honours the `ironlint` CLI exit-code contract
(`crates/ironlint-cli/src/commands/check.rs`):

| Exit | Behaviour |
|------|-----------|
| `0` (pass) | Allow. |
| `2` (block) | `return { block: true, reason }` — pi cancels the tool call. |
| `3` (internal error) | Fail-open (log + allow) by default; set `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1` to fail closed (block). |
| `4` (untrusted policy) | Fail closed: block and tell the user to review and run `ironlint trust`. |
| `1` / other (config error) | Log to stderr and allow. |

## Bash gate

In addition to file edits, this adapter gates `bash` (the agent's shell tool).
Commands that would let the agent free itself — `ironlint trust`, or a Bash
write to `.ironlint.yml` / `.ironlint/scripts/` — are denied. Ordinary commands
are not slowed: a substring pre-filter skips the decision entirely for commands
that never mention `ironlint` or `.ironlint`. The deny decision is shared across
every adapter via `ironlint gate-bash`. The branch runs before the
config-existence check, so it fires even in a project with no `.ironlint.yml` —
exactly when the agent is most motivated to self-trust. See
[The trust guide](../../docs/security/trust.md#the-agent-cant-bless-its-own-config)
for the protected paths and the shell-classification boundary.

## Known gaps (v1)

- **`edit` fuzzy-match fallback** can't be faithfully simulated, so those edits skip the check (fail-open on simulate-failure). Exact + unique `oldText` edits check normally.
- **Checks that read the file from disk** (via `$IRONLINT_FILE`) see the *pre-edit* content; only the proposed post-edit content piped on **stdin** reflects the pending change. `ironlint-disable` directives carried in that proposed content are honoured.
- **pi subagents** are not specially handled (deferred).
- **No cross-edit checks.** The check evaluates each `write` / `edit` in isolation; it does not aggregate edits across a turn.

## Diagnostic

If the check isn't firing:

1. `ironlint --version` runs on `PATH`.
2. `.ironlint.yml` is present in the project root.
3. `.ironlint.yml` is trusted: `ironlint trust`.
4. pi loaded the extension (check pi's extension discovery logs / `/reload`).
5. Run `ironlint doctor` for a structured health report.
6. Run the bundled suite against your install:

   ```bash
   PATH="$(pwd)/target/release:${PATH}" \
     node --experimental-strip-types --test adapters/pi/test/*.test.ts
   ```
