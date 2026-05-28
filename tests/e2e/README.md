# Adapter Docker e2e harness

On-demand smoke tests that run each shipping adapter (`claude-code`, `opencode`, `reasonix`) end-to-end against a real Anthropic Haiku 4.5 agent and assert that hector blocks a policy-violating edit.

This is **observability, not CI**. Failures don't gate merges — they tell you the adapter wiring broke.

## Prerequisites

- Docker daemon running
- Anthropic API key (Haiku 4.5 access)
- A workspace build: `cargo build --release -p hector-cli`

## One-time setup

```bash
cp tests/e2e/.env.e2e.example tests/e2e/.env.e2e
# Edit tests/e2e/.env.e2e and paste your ANTHROPIC_API_KEY=... value.
```

## Run

All adapters, all cases:

```bash
cargo test -p hector-e2e -- --ignored
```

One adapter:

```bash
cargo test -p hector-e2e --test claude_code -- --ignored
```

One test:

```bash
cargo test -p hector-e2e --test claude_code ast_eval_blocked -- --ignored
```

Add `--nocapture` to stream container stdout/stderr live.

## What lives where

| Path | Purpose |
|------|---------|
| `tests/e2e/base/Dockerfile` | Shared base image (Debian + Node + non-root user) |
| `tests/e2e/policy/.hector.yml` | Canonical 3-rule policy |
| `tests/e2e/cases/*.json` | Per-case prompt + target file + expected rule |
| `tests/e2e/fixture/` | Starter Node project (every run begins from this) |
| `tests/e2e/<adapter>/Dockerfile` | Per-adapter image (extends base; installs harness CLI + plugin) |
| `tests/e2e/<adapter>/drive.sh` | Container entrypoint — 6-phase lifecycle |
| `tests/e2e/<adapter>/runs/<case>/` | Forensics from the latest run (gitignored, overwritten) |
| `crates/hector-e2e/` | Rust crate: `build_image`, `run_case`, `RunResult`, assertions |

## Reading forensics

After a run, look under `tests/e2e/<adapter>/runs/<case>/`:

- `drive.log` — phase-by-phase trace from `drive.sh`
- `harness.log` — stdout/stderr of the harness CLI invocation
- `.hector/log.jsonl` — every verdict hector emitted
- `verdict.json` — the latest verdict (jq-extracted from the log)
- `workdir/<target_file>` — final state of the file under test
- `.hector.yml.from-init` — what `hector init` scaffolded (before the test policy overlaid it)

## Common failure modes

| Symptom | Likely cause |
|---|---|
| `skipping: tests/e2e/.env.e2e missing` | Run the one-time setup above. |
| `skipping: target/release/hector missing` | Run `cargo build --release -p hector-cli`. |
| `skipping: docker not on PATH` | Start Docker Desktop or install the CLI. |
| `LIFECYCLE FAIL: hector validate` | Policy file is malformed — `drive.log` has the validate output. |
| `INCONCLUSIVE: agent did not attempt the violating edit` | Model self-refused. Prompt or model may need adjustment. Not a hook bug. |
| `hook_fired(target_path=...) FAILED ... edit WAS attempted` | Real wiring bug. Adapter's hook didn't run. Check the per-adapter README and harness logs. |

## Updating the harness

| Change | What to rebuild |
|--------|-----------------|
| Edit a `cases/*.json` prompt | Nothing — case files are bind-mounted. |
| Edit `policy/.hector.yml` | Nothing. |
| Edit `<adapter>/drive.sh` | Nothing. |
| Bump harness CLI version (e.g. `claude` major bump) | `docker build -t hector-e2e-<adapter>:latest -f tests/e2e/<adapter>/Dockerfile .` |
| Bump Node base | `docker build -t hector-e2e-base:latest tests/e2e/base/` then rebuild all leaves. |

## Non-goals

- Not a CI gate.
- Not adversarial — prompts are "plausibly violating", not "deliberately evasive."
- Not a benchmark — no latency or cost measurement.
- Not pinned to a specific model — Haiku 4.5 is v1; bumping is a one-line change in `policy/.hector.yml` + each `drive.sh`.

See `docs/superpowers/specs/2026-05-27-adapter-docker-e2e-design.md` for the full design rationale.
