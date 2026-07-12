# CLI reference

Every `ironlint` subcommand and its flags. For task-oriented guides, see [Running checks](../operating/running-checks.md) and [Inspecting your config](../operating/inspecting-config.md).

The binary is `ironlint`. Run `ironlint <command> --help` for the same information at the terminal.

## `ironlint check`

Run the checks against a file or diff.

```
ironlint check [--file <path>] [--diff <path>] [--content <string|->]
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
| `--config <path>` | `.ironlint.yml` | Config file to load. |
| `--check <id>` | — | Run only this check. Repeatable; multiple flags are OR'd. |
| `--event` | `write` | What triggered the check, surfaced to checks as `$IRONLINT_EVENT`. One of `write`, `pre-commit`. |
| `--force` | off | Bypass scope matching (`files:` globs) for checks named with `--check`, so an ad-hoc `--file` outside a check's glob still runs that check. Lifecycle and `ironlint-disable:` directives still apply. Requires at least one `--check`; exits `1` if used without `--check`. |
| `--explain` | off | Print a per-check outcome report to stderr after the verdict. |
| `--allow-external-paths` | off | Allow checking files whose canonical path falls outside the config's directory. |

**Exit codes:** `0` pass · `1` config or load error · `2` block · `3` internal error · `4` untrusted config/checks (run `ironlint trust`). Argument/usage errors (a typo'd flag, a missing value, bare `ironlint`) also exit `1` — `2` is reserved exclusively for a real **Block** verdict, so a usage error is never mistaken for a policy block. Exit `4` is a config the trust layer could hash but that doesn't match (or has no entry in) the blessed store — distinct from exit `1`, which a config that fails to *parse* still uses (the trust layer can't even evaluate something that doesn't parse, so that failure defers to the same code a load error would use). Adapters must surface exit `4` loudly; pre-write adapters (every adapter as of Task 3.1) treat it as fail-closed and block the tool call. See [Running checks](../operating/running-checks.md).

## `ironlint trust`

Bless a config in the out-of-repo trust store so `ironlint check` will run it. Computes a SHA-256 over the config, every config it `extends:`, and the files under each `.ironlint/scripts/`, and records it at `~/.config/ironlint/trust.json` (keyed by the config's absolute path).

```
ironlint trust [--config <path>]
```

| Flag | Default |
|------|---------|
| `--config <path>` | `.ironlint.yml` |

See [The trust store](../security/trust.md).

## `ironlint validate`

Parse and validate the config without running any check.

```
ironlint validate [--config <path>]
```

| Flag | Default |
|------|---------|
| `--config <path>` | `.ironlint.yml` |

## `ironlint init`

Scaffold a starter `.ironlint.yml` and wire IronLint into your coding agents. Two phases:

1. **Scaffold + trust.** Detects your stack (Rust / Node / Python, including workspaces) and existing linters (biome / eslint / ruff), writes a `.ironlint.yml`, and trusts it for you. An existing config is left untouched (and not re-trusted — run [`ironlint trust`](#ironlint-trust) yourself if you hand-edit it).
2. **Wire hooks.** Detects installed agents — Claude Code, Codex, pi, OpenCode — and, after you confirm, installs IronLint's edit hook into each. Materialized hook artifacts plus a `.ironlint-adapter.json` sidecar (per-file sha256 + version) land under `~/.config/ironlint/adapters/<harness>/` for settings-hook agents, or the agent's plugin directory for plugin agents. Re-runs are idempotent.

```
ironlint init [--dir <path>] [--harness <name>]... [--global] [--yes]
            [--no-hook] [--hook-only] [--uninstall] [--dry-run]
```

| Flag | Default | Notes |
|------|---------|-------|
| `--dir <path>` | `.` | Project directory to scaffold and install into. |
| `--harness <name>` | — | Wire this agent explicitly instead of auto-detecting. Repeatable; `all` selects every supported agent. One of `claude-code`, `codex`, `pi`, `opencode`. |
| `--global` | off | Patch user-level settings (e.g. `~/.claude/settings.json`, `~/.codex/hooks.json`, `~/.pi/agent/extensions/`) instead of the project. OpenCode is always project-scoped, regardless of this flag. |
| `--yes` | off | Skip the confirmation prompt and install every detected agent. Required to wire hooks non-interactively (CI, pipes) — without a TTY and without `--yes`, init prints what it detected and installs nothing. |
| `--no-hook` | off | Scaffold and trust the config only; install no hooks. Mutually exclusive with `--hook-only`. |
| `--hook-only` | off | Skip scaffolding; only wire hooks. |
| `--uninstall` | off | Remove IronLint's hooks and materialized artifacts. Leaves `.ironlint.yml` and the trust store untouched. |
| `--dry-run` | off | Preview the hook writes and settings patches without making them. Note: the config scaffold + trust is **not** part of the dry run — it still writes and trusts `.ironlint.yml`. Pair with `--hook-only` to preview hooks without scaffolding. |

**Exit codes:** `0` on success; `3` if every attempted hook install failed; `1` on a scaffold/trust error.

## `ironlint doctor`

Diagnose the install, config, and adapter wiring. Read-only.

```
ironlint doctor [--dir <path>] [--format human|json]
```

| Flag | Default |
|------|---------|
| `--dir <path>` | `.` |
| `--format` | `human` |

**Exit codes:** `0` if every check passes or warns; `1` on any failure. See [Diagnostics](../operating/diagnostics.md).

## `ironlint explain`

Show which checks are in scope for a file and the command each would run. Read-only.

```
ironlint explain <file> [--format human|json] [--config <path>]
```

| Argument / flag | Default |
|------|---------|
| `<file>` | — (required) |
| `--format` | `human` |
| `--config <path>` | `.ironlint.yml` |

## `ironlint show-resolved-config`

Print the post-`extends:` merged check set, each check annotated by the file that defined it. Read-only.

```
ironlint show-resolved-config [--config <path>] [--format tsv|yaml|json]
```

| Flag | Default |
|------|---------|
| `--config <path>` | `.ironlint.yml` |
| `--format` | `tsv` |

See [`show-resolved-config` output](show-resolved-config.md).

## `ironlint schema`

Print the canonical check-authoring guide — the `.ironlint.yml` `{files, run}`
schema, the exit-code contract, and the common check patterns. Read-only; loads
no config. This is the same guide `ironlint init` installs into each agent as the
`ironlint-config` skill.

```
ironlint schema
```

**Exit codes:** always `0`.

## `ironlint update`

Update the `ironlint` binary in place to the latest GitHub release. Detects the install receipt the [installer](../../README.md#install) wrote and, if present, re-runs that same installer (`ironlint-cli-installer.sh` on Unix, `.ps1` on Windows), which downloads and self-replaces the running binary. The installer is idempotent, so running `update` when already current re-runs it harmlessly (exit `0`).

```
ironlint update
```

Only self-updates binaries installed via the shell/PowerShell installer. A binary from `cargo install` or a source build has no receipt; `update` then prints the command that *will* update it — the installer one-liner, or `cargo install --git … ironlint-cli --force` — and exits `1`.

**Exit codes:** `0` on a successful update; `1` on any failure, including a non-installer build that can't self-update.

## `ironlint watch`

A read-only live TUI over `.ironlint/log.jsonl`. Run it in a pane beside your
coding agent to watch checks fire in real time.

```
ironlint watch [--dir DIR]
```

- `--dir DIR` — directory containing `.ironlint.yml` / `.ironlint/log.jsonl` (default: cwd).

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

## `ironlint gate-bash`

Reads a Bash command on stdin and decides whether it may run. Exit `0` = allow
(empty stdout); exit `2` = block (reason on stdout). Used internally by every
adapter's Bash branch; you don't call it by hand.

Not a `check`: no config load, no trust gate, no per-check spawn. The
bash-gate must run even when `.ironlint.yml` is missing or untrusted — that
is exactly when the agent is most motivated to run `ironlint trust`. Any
exit other than `0` or `2` (spawn failure, signal death) is treated by the
adapters as fail-closed.

See `docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`
for the threat model and the documented known gap (variable-substitution
indirection).

## Read-only commands

`validate`, `doctor`, `explain`, `show-resolved-config`, `schema`, and `watch` never run a check or write telemetry. They exit `0` on success and `1` on a config error — never `2`. Trust is enforced only by `check`; these commands run against an unblessed config so you can debug it.
