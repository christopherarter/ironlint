# IronLint — Reasonix adapter

`PreToolUse` hook integration for [DeepSeek-Reasonix](https://esengine.github.io/DeepSeek-Reasonix/).

Runs `ironlint check --file <path> --content -` on every `write_file` or `edit_file` call **before** the edit lands on disk. Reasonix's `PreToolUse` is gating — exit 2 refuses the tool call, so a policy violation physically blocks the bad edit instead of just surfacing as a warning. (Reasonix's `PostToolUse` is non-gating; see [`specs/2026-05-25-reasonix-adapter.md`](../../specs/2026-05-25-reasonix-adapter.md) for why a `PostToolUse`-shaped hook would not work.)

## Install

```bash
ironlint init --harness reasonix
```

This patches `~/.reasonix/settings.json` (Reasonix only supports user-global
scope) to register a `PreToolUse` hook matching `^(write_file|edit_file|multi_edit)$`.
The adapter artifact is written atomically to
`~/.config/ironlint/adapters/reasonix/hook.sh` and a `.ironlint-adapter.json` sidecar
(per-file sha256 + version) is placed alongside it. A backup of the prior settings
file is saved as `<settings>.bak` on the first write; re-runs are idempotent
(unchanged → "already present", changed artifact → "updated").

Verify the install:

```bash
ironlint doctor
```

To remove the hook:

```bash
ironlint init --uninstall --harness reasonix
```

This removes the hook entry, the materialized artifact, and the sidecar from
`~/.config/ironlint/adapters/reasonix/`. Your `.ironlint.yml` and trust store are
untouched.

## Manual fallback

Use these steps if the `ironlint` binary is not available (e.g., bootstrapping a
fresh machine before you can build):

1. Build / install the `ironlint` binary:

   ```bash
   cargo install --path . # from the ironlint repo root
   ```

2. Add the hook to Reasonix's global settings (`~/.reasonix/settings.json`) or a project-local override (`<project>/.reasonix/settings.json`). Project scope takes precedence.

   Copy `hooks/settings.example.json` into the target settings file, merging with any existing `hooks` keys.

3. In each project you want ironlint to check, run `ironlint init && ironlint trust` to scaffold and fingerprint a `.ironlint.yml`.

The hook is a silent no-op in any project that lacks `.ironlint.yml`, so installing globally is safe.

## Requirements

- `ironlint` on `PATH`
- `jq` on `PATH` (parses the Reasonix stdin payload)
- `python3` on `PATH` (the `edit_file` path shells out to it to synthesize post-edit content)
- `bash`

## How it works

| Tool | Source of proposed content | Gating |
| --- | --- | --- |
| `write_file` | `toolArgs.content` (verbatim) | exit 2 blocks |
| `edit_file` | synthesize from `(path, search, replace)`; fail closed if `search` is not unique | exit 2 blocks |
| `multi_edit` | not currently gated (no-op) | follow-up; see spec §9.3 |

Per-edit content reaches ironlint via stdin (`--content -`), keeping argv free of large payloads. The `--file` path is the real on-disk path so scope globs, baseline matching, and AST language detection all key off the project's actual layout — not a tempfile.

### Checks and pre-write gating

Checks receive the proposed content on **stdin**. Write the check's `run` command in `.ironlint.yml` and the check evaluates the proposed edit before it lands on disk — e.g.:

```yaml
run: "biome check --stdin-file-path=$IRONLINT_FILE"
run: "ruff check --stdin-filename $IRONLINT_FILE -"
run: "eslint --stdin --stdin-filename $IRONLINT_FILE"
```

`$IRONLINT_FILE` is a path/extension hint (for config lookup and language detection); the content comes from stdin. A path-only command (`biome check $IRONLINT_FILE`) still reads the on-disk file and is silently wrong under PreToolUse.

**Per-tool boundary (not per-harness):** stdin-capable single-file tools (biome, eslint, ruff, prettier, shellcheck, …) can check pre-write. Whole-program tools — tsc, cargo, test runners, anything that needs the full project tree — cannot check a single proposed file meaningfully; run those post-write or in CI. This boundary is a property of the tool, not of this adapter or Reasonix.

### Limitation: `bash` tool shell-out

A `bash` tool call that writes a file via `cat > foo.ts` (or any shell redirection) does not match `write_file`/`edit_file` and bypasses the hook entirely. This is a known gap; matching `bash` and parsing arbitrary commands is too brittle to attempt here.

## Differences from the Claude Code adapter

| | Claude Code | Reasonix |
| --- | --- | --- |
| Settings file | `hooks/hooks.json` shipped with plugin | `~/.reasonix/settings.json` (user-edited) |
| Plugin root env | `${CLAUDE_PLUGIN_ROOT}` | none — use absolute paths |
| Gating lifecycle event | `PostToolUse` (blocks) | `PreToolUse` (blocks) |
| stdin field for path | `tool_input.file_path` | `toolArgs.path` |
| Edit tool names | `Edit`, `Write` | `edit_file`, `write_file`, `multi_edit` |

Both adapters run per-file `ironlint check`; ironlint is a static check runner.
