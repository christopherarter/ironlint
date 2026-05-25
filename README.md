# Hector

Policy-enforcement pipeline for AI coding agents. Rust rewrite of [dynamik-dev/bully](https://github.com/dynamik-dev/bully).

## Status

0.2 (in progress). Engines: `script`, `ast`, `semantic` (Anthropic + OpenRouter + Ollama), `session`. CLI: `check`, `trust`, `validate`, `init`, `migrate`, `baseline`, `session record`, `doctor`. Claude Code + OpenCode adapters shipped. See [`docs/doctor.md`](docs/doctor.md) for the diagnostic schema.

## Adapters

- **Claude Code** — `adapters/claude-code/`. PostToolUse + Stop hooks, three skills. See [docs/adapters/claude-code.md](docs/adapters/claude-code.md).
- **OpenCode** — `adapters/opencode/`. `tool.execute.after` + `event` (`session.created` / `session.idle`) plugin. See [docs/adapters/opencode.md](docs/adapters/opencode.md).
- *Aider, pre-commit, MCP — planned for 0.2/0.3.*

## Build

```bash
cargo build --release
./target/release/hector --version
```

## Quick start

See [docs/quickstart.md](docs/quickstart.md).

## Inspect

Read-only commands that never run engines, call LLMs, or write telemetry. Exit `0` on success, `1` on config error — never `2`.

- `hector explain <file>` — show which rules are in scope for a file and which scope glob matched (or which skip pattern suppressed it). `--format human|json` (default `human`).
- `hector guide <file>` — list rules whose scope matches the file with their severity and description. `--format human|json` (default `human`).
- `hector show-resolved-config [--format tsv|yaml|json]` — print the post-`extends:` merged rule set, with each rule annotated by the file that defined it. See [docs/show-resolved-config.md](docs/show-resolved-config.md).

All three honor `--config <path>` (default `.hector.yml`).

## Baseline

`hector baseline record` snapshots current violations so future `hector check` runs suppress them (noise reduction for pre-existing issues). `hector baseline refresh` re-hashes each entry against current file content and drops entries whose line is gone.

**File-level violations now require content match.** Since A1 (0.2),
baselined `line: None` violations are matched on both their fingerprint
and a normalized hash of the violation message. Old (v2) baselines
continue to match on fingerprint alone during a grace period — run
`hector baseline refresh` to upgrade. Normalization strips ISO-8601
timestamps and ANSI color escapes.

## Specs

- [`specs/overview.md`](specs/overview.md) — Hector at 1.0
- [`specs/2026-05-11-hector-plan-and-0.1-design.md`](specs/2026-05-11-hector-plan-and-0.1-design.md) — plan + 0.1 design
