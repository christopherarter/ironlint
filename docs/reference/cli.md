# CLI reference

Every `hector` subcommand and its flags. For task-oriented guides, see [Running checks](../operating/running-checks.md) and [Inspecting your config](../operating/inspecting-config.md).

The binary is `hector`. Run `hector <command> --help` for the same information at the terminal.

## `hector check`

Run the checks against a file or diff.

```
hector check [--file <path>] [--diff <path>] [--content <string|->]
             [--format human|json] [--config <path>] [--check <id>]...
             [--event write|pre-commit] [--force] [--explain]
             [--allow-external-paths]
```

| Flag | Default | Notes |
|------|---------|-------|
| `--file <path>` | — | File to check. |
| `--diff <path>` | — | Unified diff; each changed file is checked. |
| `--content <string\|->` | — | Proposed post-edit content to evaluate instead of reading `--file` from disk; `-` reads it from stdin. Requires `--file`; conflicts with `--diff`. |
| `--format` | `human` | `human` or `json`. See [Verdict JSON](verdict-json.md). |
| `--config <path>` | `.hector.yml` | Config file to load. |
| `--check <id>` | — | Run only this check. Repeatable; multiple flags are OR'd. |
| `--event` | `write` | What triggered the check, surfaced to checks as `$HECTOR_EVENT`. One of `write`, `pre-commit`. |
| `--force` | off | Bypass scope matching (`files:` globs) for checks named with `--check`, so an ad-hoc `--file` outside a check's glob still runs that check. Lifecycle and `hector-disable:` directives still apply. Requires at least one `--check`; exits `1` if used without `--check`. |
| `--explain` | off | Print a per-check outcome report to stderr after the verdict. |
| `--allow-external-paths` | off | Allow checking files whose canonical path falls outside the config's directory. |

**Exit codes:** `0` pass · `1` config or load error · `2` block · `3` internal error. See [Running checks](../operating/running-checks.md).

## `hector trust`

Bless a config in the out-of-repo trust store so `hector check` will run it. Computes a SHA-256 over the config, every config it `extends:`, and the files under each `.hector/gates/`, and records it at `~/.config/hector/trust.json` (keyed by the config's absolute path).

```
hector trust [--config <path>]
```

| Flag | Default |
|------|---------|
| `--config <path>` | `.hector.yml` |

See [The trust store](../security/trust.md).

## `hector validate`

Parse and validate the config without running any check.

```
hector validate [--config <path>]
```

| Flag | Default |
|------|---------|
| `--config <path>` | `.hector.yml` |

## `hector init`

Scaffold a starter `.hector.yml` and wire Hector into your coding agents. Two phases:

1. **Scaffold + trust.** Detects your stack (Rust / Node / Python, including workspaces) and existing linters (biome / eslint / ruff), writes a `.hector.yml`, and trusts it for you. An existing config is left untouched (and not re-trusted — run [`hector trust`](#hector-trust) yourself if you hand-edit it).
2. **Wire hooks.** Detects installed agents — Claude Code, Reasonix, pi, OpenCode — and, after you confirm, installs Hector's edit hook into each. Materialized hook artifacts plus a `.hector-adapter.json` sidecar (per-file sha256 + version) land under `~/.config/hector/adapters/<harness>/` for settings-hook agents, or the agent's plugin directory for plugin agents. Re-runs are idempotent.

```
hector init [--dir <path>] [--harness <name>]... [--global] [--yes]
            [--no-hook] [--hook-only] [--uninstall] [--dry-run]
```

| Flag | Default | Notes |
|------|---------|-------|
| `--dir <path>` | `.` | Project directory to scaffold and install into. |
| `--harness <name>` | — | Wire this agent explicitly instead of auto-detecting. Repeatable; `all` selects every supported agent. One of `claude-code`, `reasonix`, `pi`, `opencode`. |
| `--global` | off | Patch user-level settings (e.g. `~/.claude/settings.json`, `~/.pi/agent/extensions/`) instead of the project. Reasonix is always user-global; OpenCode is always project-scoped, regardless of this flag. |
| `--yes` | off | Skip the confirmation prompt and install every detected agent. Required to wire hooks non-interactively (CI, pipes) — without a TTY and without `--yes`, init prints what it detected and installs nothing. |
| `--no-hook` | off | Scaffold and trust the config only; install no hooks. Mutually exclusive with `--hook-only`. |
| `--hook-only` | off | Skip scaffolding; only wire hooks. |
| `--uninstall` | off | Remove Hector's hooks and materialized artifacts. Leaves `.hector.yml` and the trust store untouched. |
| `--dry-run` | off | Preview the hook writes and settings patches without making them. Note: the config scaffold + trust is **not** part of the dry run — it still writes and trusts `.hector.yml`. Pair with `--hook-only` to preview hooks without scaffolding. |

**Exit codes:** `0` on success; `3` if every attempted hook install failed; `1` on a scaffold/trust error.

## `hector doctor`

Diagnose the install, config, and adapter wiring. Read-only.

```
hector doctor [--dir <path>] [--format human|json]
```

| Flag | Default |
|------|---------|
| `--dir <path>` | `.` |
| `--format` | `human` |

**Exit codes:** `0` if every check passes or warns; `1` on any failure. See [Diagnostics](../operating/diagnostics.md).

## `hector explain`

Show which checks are in scope for a file and the command each would run. Read-only.

```
hector explain <file> [--format human|json] [--config <path>]
```

| Argument / flag | Default |
|------|---------|
| `<file>` | — (required) |
| `--format` | `human` |
| `--config <path>` | `.hector.yml` |

## `hector show-resolved-config`

Print the post-`extends:` merged check set, each check annotated by the file that defined it. Read-only.

```
hector show-resolved-config [--config <path>] [--format tsv|yaml|json]
```

| Flag | Default |
|------|---------|
| `--config <path>` | `.hector.yml` |
| `--format` | `tsv` |

See [`show-resolved-config` output](show-resolved-config.md).

## `hector schema`

Print the canonical check-authoring guide — the `.hector.yml` `{files, run}`
schema, the exit-code contract, and the common check patterns. Read-only; loads
no config. This is the same guide `hector init` installs into each agent as the
`hector-config` skill.

```
hector schema
```

**Exit codes:** always `0`.

## `hector update`

Update the `hector` binary in place to the latest GitHub release. Reads the install receipt the [installer](../../README.md#install) wrote, checks the latest release, and — when there's a newer one — downloads and re-runs the same installer, then self-replaces the running binary. A no-op when you're already current.

```
hector update
```

Only self-updates binaries installed via the shell/PowerShell installer. A binary from `cargo install` or a source build has no receipt; `update` then prints the command that *will* update it — the installer one-liner, or `cargo install --git … hector-cli --force` — and exits `1`.

**Exit codes:** `0` on a successful update or when already current; `1` on any failure, including a non-installer build that can't self-update.

## `hector watch`

A read-only live TUI over `.hector/log.jsonl`. Run it in a pane beside your
coding agent to watch checks fire in real time.

```
hector watch [--dir DIR]
```

- `--dir DIR` — directory containing `.hector.yml` / `.hector/log.jsonl` (default: cwd).

Two views, toggled with `Tab` / `→` / `←`:

- **Stream** — newest-first feed of check runs: time, ✓/✗/⚠, file (or
  `pre-commit · N files`), elapsed, and the `write`/`commit` event. Blocked rows
  show the failing check and `write rejected`; internal-error rows show the
  reason. The failure *message* is not shown — it isn't stored in the log
  (the agent receives it live via the hook).
- **Explorer** — whole-log aggregate: runs / blocks / internal / pass%, and a
  per-check table ranked by blocks with block-rate and p50 latency. `↑`/`↓`
  select a check; `↵` jumps to the Stream filtered to it.

`q` / `Esc` quits. Requires an interactive terminal; in a non-TTY it exits `1`
with a hint. Read-only: it runs no checks, writes no telemetry, and does not
enforce trust.

## Read-only commands

`validate`, `doctor`, `explain`, `show-resolved-config`, `schema`, and `watch` never run a check or write telemetry. They exit `0` on success and `1` on a config error — never `2`. Trust is enforced only by `check`; these commands run against an unblessed config so you can debug it.
