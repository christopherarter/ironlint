# Hector

Policy-enforcement pipeline for AI coding agents. Rust rewrite of [dynamik-dev/bully](https://github.com/dynamik-dev/bully).

## Status

0.2 (in progress). Engines: `script`, `ast`. CLI: `check`, `trust`, `validate`, `init`, `migrate`, `baseline`, `doctor`. Claude Code, OpenCode, Reasonix, and pi adapters shipped. See [`docs/operating/diagnostics.md`](docs/operating/diagnostics.md) for the diagnostic schema.

## Install

Prebuilt binaries for macOS (Apple Silicon and Intel), Linux (x86-64), and Windows (x86-64). The installer downloads the right binary, drops it in `~/.cargo/bin`, and puts it on your `PATH` — no Rust toolchain required:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/christopherarter/hector/releases/latest/download/hector-cli-installer.sh | sh
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/christopherarter/hector/releases/latest/download/hector-cli-installer.ps1 | iex"
```

Or build from source (needs a Rust toolchain):

```sh
cargo install --git https://github.com/christopherarter/hector hector-cli
```

Then run `hector --version`.

## Documentation

Full docs are in [`docs/`](docs/README.md) — start with [Getting started](docs/getting-started.md), the [Visual elevator pitch](docs/visual-elevator-pitch.md), or the [Architecture diagram](docs/architecture.md).

## Adapters

- **Claude Code** — `adapters/claude-code/`. PostToolUse hook, three skills. See [docs/adapters/claude-code.md](docs/adapters/claude-code.md).
- **OpenCode** — `adapters/opencode/`. `tool.execute.before` gates proposed edits against `hector check`. See [docs/adapters/opencode.md](docs/adapters/opencode.md).
- **Reasonix** — `adapters/reasonix/`. PreToolUse hook for `write_file` / `edit_file`. See [adapters/reasonix/README.md](adapters/reasonix/README.md).
- **pi** — `adapters/pi/`. `tool_call` hook gates proposed edits before they're written. See [adapters/pi/README.md](adapters/pi/README.md).
- *Aider, pre-commit, MCP — planned for 0.2/0.3.*

## Build

```bash
cargo build --release
./target/release/hector --version
```

## Quick start

See [docs/getting-started.md](docs/getting-started.md).

## Exit codes (`hector check`)

| Code | Meaning |
|------|---------|
| 0 | Pass or Warn — all rules evaluated cleanly |
| 1 | Config error — untrusted fingerprint, parse failure, missing file |
| 2 | Block — ≥1 error-severity policy violation |
| 3 | InternalError — ≥1 engine runtime error (`__internal` violations present); e.g. AST refused diff, script spawn failure |

Adapters fail-open on exit 3 by default. Opt-in fail-closed: `HECTOR_FAIL_CLOSED_ON_INTERNAL=1`.

## Inspect

Read-only commands that never run engines or write telemetry. Exit `0` on success, `1` on config error — never `2`.

- `hector explain <file>` — show which rules are in scope for a file and which scope glob matched (or which skip pattern suppressed it). `--format human|json` (default `human`).
- `hector guide <file>` — list rules whose scope matches the file with their severity and description. `--format human|json` (default `human`).
- `hector show-resolved-config [--format tsv|yaml|json]` — print the post-`extends:` merged rule set, with each rule annotated by the file that defined it. See [docs/reference/show-resolved-config.md](docs/reference/show-resolved-config.md).

All three honor `--config <path>` (default `.hector.yml`).

## Baseline

`hector baseline record` snapshots current violations so future `hector check` runs suppress them (noise reduction for pre-existing issues). `hector baseline refresh` re-hashes each entry against current file content and drops entries whose line is gone.

**File-level violations now require content match.** Since A1 (0.2),
baselined `line: None` violations are matched on both their fingerprint
and a normalized hash of the violation message. Old (v2) baselines
continue to match on fingerprint alone during a grace period — run
`hector baseline record` to re-record entries under the new schema.
Normalization strips ISO-8601 timestamps and ANSI color escapes.

## Specs

- [`specs/overview.md`](specs/overview.md) — Hector at 1.0
- [`specs/2026-05-11-hector-plan-and-0.1-design.md`](specs/2026-05-11-hector-plan-and-0.1-design.md) — plan + 0.1 design
