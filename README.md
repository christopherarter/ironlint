<p align="center">
  <img src="iron-lint.png" alt="IronLint" width="500" />
</p>

<h1 align="center">IronLint: Checks Your Coding Agent Can't Skip</h1>

<p align="center">
  <strong>Every edit your agent makes runs your checks first. Any nonzero exit blocks the write. The check owns the verdict — no engines, no severities, no DSL.</strong>
</p>

<p align="center">
  <a href="https://github.com/christopherarter/ironlint/actions/workflows/ci.yml"><img src="https://github.com/christopherarter/ironlint/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <a href="https://github.com/christopherarter/ironlint/releases/latest"><img src="https://img.shields.io/github/v/release/christopherarter/ironlint?label=release" alt="Latest release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache_2.0-green" alt="Apache 2.0" /></a>
  <img src="https://img.shields.io/badge/built_with-Rust-orange" alt="Built with Rust" />
  <img src="https://img.shields.io/badge/agents-Claude_Code_·_OpenCode_·_Reasonix_·_pi-5A67D8" alt="Agent adapters" />
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#connect-your-agent">Connect your agent</a> ·
  <a href="#configuration">Configuration</a> ·
  <a href="#what-ironlint-does">What it does</a> ·
  <a href="#why-this-works">Why This Works</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="docs/README.md">Docs</a>
</p>

---

IronLint is local CI for AI coding agents. GitHub Actions runs your checks in the cloud after you push; IronLint runs the same kind of checks locally, on every edit your agent makes, **before the code lands — and it can refuse.** A check is a file glob plus a shell command. IronLint hands that command your proposed content on stdin and reads one thing back: the exit code. Any nonzero exit (1–125) blocks the write. No engines, no severities, no output parsing — the check owns the decision.

New here? Start with [Getting started](docs/getting-started.md).

## Install

Prebuilt binaries for macOS (Apple Silicon and Intel), Linux (x86-64), and Windows (x86-64). The installer downloads the right binary, drops it in `~/.cargo/bin`, and puts it on your `PATH` — no Rust toolchain required:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/christopherarter/ironlint/releases/latest/download/ironlint-cli-installer.sh | sh
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/christopherarter/ironlint/releases/latest/download/ironlint-cli-installer.ps1 | iex"
```

Or build from source (needs a Rust toolchain):

```sh
cargo install --git https://github.com/christopherarter/ironlint ironlint-cli
```

Then run `ironlint --version`. Update any time with `ironlint update` (installer-based installs; source builds update with `cargo install … --force`).

## Connect your agent

`ironlint init` takes you from clone to gated in one command. It scaffolds a starter `.ironlint.yml`, trusts it, detects your coding agents, and wires IronLint's edit hook into each — so policy runs on every edit without you calling `ironlint check` by hand:

```sh
ironlint init
```

It detects Claude Code, Reasonix, pi, and OpenCode, asks before touching anything, and installs the hook. Target one explicitly, wire them all, or patch your user account instead of the project:

```sh
ironlint init --harness opencode   # just one agent
ironlint init --harness all        # every supported agent
ironlint init --global             # user-level settings, not the project
```

It also installs an `ironlint-config` authoring skill so the agent knows how to write checks; run `ironlint schema` to read the format yourself. `ironlint doctor` verifies the wiring (one row per agent); `ironlint init --uninstall --harness <name>` removes it.

## Configuration

One YAML file at `.ironlint.yml` in your repo root. A map of checks; each names what to match, where it applies, and the command that decides.

```yaml
# .ironlint.yml
checks:
  no-console:
    files: "**/*.ts"
    run: "! grep -n 'console.log'"    # proposed content arrives on stdin

  lint-and-format:
    files: "src/**/*.py"
    on: [write, pre-commit]           # write: per file; pre-commit: once, with $IRONLINT_FILES
    steps:
      - name: ruff
        run: "ruff check --quiet --stdin-filename \"$IRONLINT_FILE\" -"
      - name: no-todo
        run: "! grep -n 'TODO' $IRONLINT_FILES"
```

A check blocks by exiting nonzero (1–125) and owns its own message. There are no engines, no `severity`, no output-parsing modes.

Sharing checks across repos:

```yaml
extends: ["../shared/ironlint-base.yml"]

checks:
  # project-specific overrides + additions
```

Local checks override inherited checks of the same id. Full format: `ironlint schema` or [Configuration](docs/getting-started.md).

## What IronLint does

- **Runs on the write, not after.** Every agent edit fires your checks with the proposed content on stdin, before it lands on disk. A nonzero exit refuses the write. GitHub Actions can't reach that early.
- **The check owns the verdict.** A check is a file glob plus a shell command (or `steps:`). IronLint reads one thing back — the exit code. `0` passes, `1–125` blocks, and your command's stdout/stderr becomes the block message. No engines, no severities, no output parsing.
- **Your existing linters, unchanged.** `ruff`, `eslint`, `tsc`, `clippy`, `biome` — the same checks CI runs, wired in as-is. No per-tool integration to write.
- **Two lifecycles, one config.** `on: [write]` fires per edit; `on: [pre-commit]` fires once over the whole staged set. A single check can do both — IronLint keys by check, not by event.
- **Trust before it runs.** IronLint refuses an untrusted config. `ironlint trust` blesses it; any change to the config or its gate scripts revokes trust until you re-bless. `check` fails closed on untrusted input.
- **Telemetry, no setup.** Every run appends a JSONL record to `.ironlint/log.jsonl`; the directory ignores itself. `ironlint-review` flags noisy and dead checks.

## Why This Works

An LLM catches its own slop poorly, but fixes it well once something reliable points at the problem. IronLint is the deterministic half of that loop: a check runs on every edit and hands back a signal the model can't talk its way around — a nonzero exit. The agent writes, the check judges, the agent fixes. Push every fixed-rule quality gate onto deterministic checks, and leave prompts and skills for the judgment calls that genuinely need them.

That ethos has a name and a longer argument — [Determinism-in-the-loop](https://arter.dev/blog/the-antidote-to-code-slop/).

## Lifecycles

A check fires on `write` (default), `pre-commit`, or both.

| Event | Trigger | stdin | `$IRONLINT_FILE` | `$IRONLINT_FILES` |
|-------|---------|-------|------------------|-------------------|
| `write` | Every agent edit | proposed content | the edited file | same, single entry |
| `pre-commit` | Once before a commit | empty | not set | all staged files, newline-joined |

Use `on: [write, pre-commit]` to fire at both — no duplication. This is the inversion that separates IronLint from lefthook, whose earliest reach is `pre-commit`, after the agent already wrote the file.

## The check ABI

Every check receives, on its environment:

- `$IRONLINT_FILE` — absolute path of the file under check (set for `write`; not set for `pre-commit`).
- `$IRONLINT_FILES` — newline-joined list of all files (single entry for `write`; all staged files for `pre-commit`).
- `$IRONLINT_ROOT` — project root (the check's cwd).
- `$IRONLINT_EVENT` — `write` or `pre-commit`.
- `$IRONLINT_TMPFILE` — a materialized temp file holding the proposed content, set only when your `run`/`steps` reference it.
- **stdin** — proposed post-edit content (`write`) or empty (`pre-commit`).

Read proposed content from **stdin**, not from `$IRONLINT_FILE`. On harnesses that gate before the write lands (reasonix, pi), the file on disk still holds the old content.

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│           Agent Edit / Write   (or git pre-commit)           │
│                            ↓                                   │
│                  harness adapter hook                         │
│                            ↓                                   │
│                     ironlint check                            │
│                            ↓                                   │
│      match touched file → checks whose `files:` glob hits     │
│                            ↓                                   │
│   for each check:  sh -c <run>                                │
│     env:   $IRONLINT_FILE  $IRONLINT_ROOT  $IRONLINT_EVENT …  │
│     stdin: proposed post-edit content                         │
│                            ↓                                   │
│                  read only the exit code                      │
│        0 → pass          1–125 → BLOCK the write              │
│   126 / 127 / timeout / signal → InternalError (never a       │
│                                  silent pass)                  │
└──────────────────────────────────────────────────────────────┘
```

Each adapter collapses one harness's edit hook into the ABI above and runs `ironlint check`. The runner matches the touched file to checks, dispatches each (once per file on `write`, once per check on `pre-commit`), and folds the exit codes into a single verdict.

## Reference

<details>
<summary>Exit codes (<code>ironlint check</code>)</summary>

| Code | Meaning |
|------|---------|
| 0 | Pass — every matched check passed |
| 1 | Config or load error — parse failure, missing file |
| 2 | Block — at least one check exited nonzero (1–125) |
| 3 | InternalError — at least one check crashed (not found, timeout, killed by signal) |
| 4 | Untrusted config/gates — run `ironlint trust` |

Adapters fail-open on exit 3 by default. Opt-in fail-closed: `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`. Exit 4 is different: adapters must surface it loudly, and pre-write adapters (every shipped adapter) treat it as fail-closed and block the tool call outright — an untrusted config is never silently allowed through.
</details>

<details>
<summary>Inspect (read-only commands)</summary>

Never run a check or write telemetry. Exit `0` on success, `1` on a config error — never `2`.

```bash
ironlint explain <file>          # which checks are in scope for a file, and their run commands
ironlint show-resolved-config    # the post-extends: merged check set, annotated by source
ironlint schema                  # the check-authoring guide
```

All honor `--config <path>` (default `.ironlint.yml`).
</details>

<details>
<summary>Disable a check</summary>

Add `# ironlint-disable: <check-id>` anywhere in a file to suppress that check for the whole file.
</details>

<details>
<summary>Adapters</summary>

- **Claude Code** — `adapters/claude-code/`. PostToolUse hook, plus three skills. See [docs/adapters/claude-code.md](docs/adapters/claude-code.md).
- **OpenCode** — `adapters/opencode/`. `tool.execute.before` gates proposed edits. See [docs/adapters/opencode.md](docs/adapters/opencode.md).
- **Reasonix** — `adapters/reasonix/`. PreToolUse hook for `write_file` / `edit_file`. See [adapters/reasonix/README.md](adapters/reasonix/README.md).
- **pi** — `adapters/pi/`. `tool_call` hook gates proposed edits before they're written. See [adapters/pi/README.md](adapters/pi/README.md).
- *Aider, pre-commit, MCP — planned.*
</details>

<details>
<summary>vs lefthook</summary>

At the `pre-commit` boundary, IronLint is a near line-for-line swap for lefthook's gate role:

```
# lefthook.yml                          # .ironlint.yml
pre-commit:                             checks:
  commands:                               prettier:
    prettier:                               files: '**/*.{ts,css,md}'
      glob: "*.{ts,css,md}"                 on: [pre-commit]
      run: prettier --check {staged_files}  run: prettier --check $IRONLINT_FILES
```

**Absorbed:** the gate role. **Declined:** `parallel`, `stage_fixed`, and the fixer/restager half. **Added:** the `write` lifecycle — lefthook's earliest reach is `pre-commit`, after the agent already wrote the file; IronLint fires on the write itself.
</details>

<details>
<summary>Build from source</summary>

```bash
cargo build --release
./target/release/ironlint --version
```
</details>

## Documentation

Full docs live in [`docs/`](docs/README.md) — start with [Getting started](docs/getting-started.md) or the [Architecture](docs/architecture.md) page.

## Contributing

Issues and PRs welcome. Good places to start: a new adapter under `adapters/`, a check pack under `tests/fixtures/`, or docs under `docs/`. See [AGENTS.md](AGENTS.md) for build, test, and coverage conventions.

## License

Apache 2.0. See [LICENSE](LICENSE).
