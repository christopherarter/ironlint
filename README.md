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

## Specs

- [`specs/overview.md`](specs/overview.md) — Hector at 1.0
- [`specs/2026-05-11-hector-plan-and-0.1-design.md`](specs/2026-05-11-hector-plan-and-0.1-design.md) — plan + 0.1 design
