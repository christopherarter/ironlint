# Hector

Policy-enforcement pipeline for AI coding agents. Rust rewrite of [dynamik-dev/bully](https://github.com/dynamik-dev/bully).

## Status

0.1b. Engines: `script`, `ast`, `semantic` (Anthropic), `session`. Commands: `check`, `trust`, `validate`, `init`, `migrate`, `baseline`. Plan C (Claude Code adapter) is next.

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
