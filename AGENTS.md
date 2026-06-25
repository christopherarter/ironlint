# AGENTS.md

Guidance for AI coding agents working in this repo.

## What this is

Rust rewrite of [dynamik-dev/bully](https://github.com/dynamik-dev/bully) — a tool-agnostic, static policy-enforcement gate for AI coding agents. Status: **0.3 "gates" redesign, Plans 1 (core engine + CLI) and 2 (trust store) merged.** A gate is `files` (globs) + `run` (a shell command); hector matches a touched file to gates, runs each `run` once with the ABI on env + proposed content on stdin, and reads only the exit code (`2` = Block). No per-rule engines, no severity, no LLM. CLI ships `check`, `validate`, `init`, `explain`, `show-resolved-config`, `doctor`, `trust` (blesses the out-of-repo store; `check` fails closed — exit 1 — on untrusted config/gates). Claude Code adapter at `adapters/claude-code/` (predates the redesign; adapter ABI is Plan 4). Authoritative design: `specs/2026-06-15-hector-gates-redesign-design.md` (supersedes the old engine model in `specs/2026-05-11-hector-plan-and-0.1-design.md`); per-phase plans in `plans/` (`plans/2026-06-15-hector-gates-redesign-core.md` = Plan 1).

**Not yet built (later plans):** `hector verify` + the full `doctor` expansion (Plan 3 — `doctor` is currently a minimal static-check command); the adapter `--event`/ABI side (Plan 4).

## Rules

- Bug fixes start with a failing test (use the test-writing skill). The failing test becomes regression coverage.
- After completing a coding task, request code review from a separate agent.
- Your code reviews are reviewed by the principal engineer — do deep work.
- Tool hasn't shipped; no hedging.
- Rust files under `crates/*/src/` must meet ≥80% **region** coverage (distinct decision points — branches, short-circuits, match arms — not executed lines). CI enforces per-file via `scripts/ci-coverage.sh` (cargo-llvm-cov). Code added without bringing the file to the gate breaks the build.
- Cognitive complexity per function is capped at **15** via clippy (`clippy.toml`, with `#![warn(clippy::cognitive_complexity)]` at each crate root). Refactor over annotate; reach for `#[allow(clippy::cognitive_complexity)]` only when complexity is intrinsic to the function and decomposing would scatter the flow — document why.
- Mutation testing is a **local, ad-hoc** investigative tool, not a CI gate (would burn runner minutes). `cargo install cargo-mutants` once, then point it at a file or diff: `cargo mutants --file 'crates/hector-core/src/<name>.rs'` for one file, or `git diff main.. > pr.diff && cargo mutants --in-diff pr.diff` for the PR. A surviving mutant means tests executed the code but didn't verify what it does — treat survivors in code you touched as a coverage gap.
- Clean up build artifacts you produced once the task is done. If you ran `cargo build --release` or built a one-off binary to verify behavior, drop it with `cargo clean -p <crate>` or `rm target/release/<bin>` after verification. Same for throwaway files like `pr.diff`, ad-hoc tarballs, scratch `cargo mutants` output, or any binary built for a single check. The persistent `target/` you're actively iterating in stays — this rule is about artifacts *this task* created, not the working tree.

## Commands

```bash
cargo build --release                       # produces ./target/release/hector
cargo test                                  # all workspace tests
cargo test -p hector-core                   # core only
cargo test -p hector-cli                    # CLI only
cargo test --test cli_e2e_gates             # single integration test file
cargo test <name>                           # filter by test-fn name
cargo clippy --all-targets -- -D warnings   # lint
cargo fmt
bash scripts/ci-coverage.sh                 # per-file ≥80% region-coverage gate (matches CI)
```

CLI tests use `assert_cmd` against the compiled binary. (`insta` snapshots may exist for some surfaces — `cargo insta review` after an intentional shape change.)

## Architecture

Cargo workspace, two crates:

- **`hector-core`** — library. Modules:
  - `config` — parse the gates YAML (`Config { extends, execution, gates }`, `Gate { files, run }`), glob scope matching (`scope.rs`), `extends:` resolution (`extends.rs`)
  - `diff` — unified-diff parser (used by CLI `--diff` to enumerate changed files)
  - `engine` — the single gate-execution model: `gate::run_gate` spawns `sh -c <run>`, feeds stdin, enforces the timeout, and classifies the exit code into a `GateOutcome` (`Pass` / `Block { message }` / `Internal(InternalReason)`). No `RuleEngine` trait, no per-engine impls.
  - `runner` — orchestrates: load → `extends`-resolve → build per-gate scope matchers → for each gate run `run_gate` per matching file → fold into a `Verdict` → telemetry-log
  - `trust` — out-of-repo allow-list at `~/.config/hector/trust.json` (XDG: `$XDG_CONFIG_HOME/hector/trust.json`). Hash covers the config bytes + every file under `.hector/gates/` (sorted by relative path); keyed by the config's canonical absolute path. Atomic write on `hector trust`. Enforcement is at the CLI `check` layer only — `HectorEngine::load` stays pure.
  - `verdict` — `Status` (Pass / Block / InternalError) + the locked JSON shape (`Verdict { blocks, errors, passed, .. }`, `Block`, `GateError`)
  - `disable` — `hector-disable: <gate-id>` line directives; file-wide (a directive anywhere suppresses that gate for that file). Directive ends at whitespace/`*`/`/`.
  - `telemetry` — `.hector/log.jsonl`, append-only check log of `PerGateRecord`s
- **`hector-cli`** — thin binary, name `hector`. `cli.rs` defines clap subcommands; `commands/{check,validate,init,explain,show_resolved_config,doctor,trust}.rs` are one-function adapters into core.

`HectorEngine::load` (`crates/hector-core/src/runner.rs`) resolves `extends` and builds the per-gate scope matchers. Two things it relies on:

1. **Extends.** `config::extends::resolve` does a cycle-detected DFS; inherited gates fill gaps but **local gates win on collision**.
2. **Legacy rejection.** `config::parser` rejects any pre-0.3 config (top-level `schema_version:`, `rules:`, or `trust:`) with a curated error pointing at the gates format — there is no migration path (no install base).

(Trust is enforced at the CLI `check` layer — `check::run` calls `trust::ensure_trusted` before invoking the engine and exits 1 on missing/mismatch. `HectorEngine::load` stays pure; read-only commands do not enforce trust.)

**The gate ABI** (locked stability surface — every adapter must satisfy it, every gate `run` may rely on it): `$HECTOR_FILE` (absolute path under check), `$HECTOR_ROOT` (project root = the gate's cwd), `$HECTOR_EVENT` (`edit`/`write`/`pre-commit`/`manual`), the proposed post-edit content on **stdin**. No string templating — the path travels only as an env value, never spliced into `run`.

**Gate verdict contract.** The gate owns the verdict via its exit code: `2` → Block; `0`/`1`/`3`–`125` → Pass (blocking is opt-in per gate — a tool that exits 1 on findings is a pass unless the script remaps it to `2`); `126`/`127`/`≥128` (signal) / wall-clock timeout → InternalError (a broken gate is never a silent pass). On Block, the gate's combined trimmed stdout+stderr is the message; if both are empty, the runner fills `"<gate-id> blocked"`.

**Exit-code contract** (`commands/check.rs`) — consumed by CI and editor adapters, do not break:

- `0` — Pass (no warning tier exists)
- `1` — config/load error (parse failure, missing file, unknown `--gate`, or untrusted config/gates)
- `2` — Block (≥1 gate exited 2)
- `3` — InternalError (≥1 gate crashed: 127 / timeout / signal)

Adapters fail-open on exit 3 by default; opt-in fail-closed via `HECTOR_FAIL_CLOSED_ON_INTERNAL=1`.

**Verdict JSON** (`verdict.rs`): `SCHEMA_VERSION = 4`. Treat `Verdict`, `Block`, `GateError`, `Status`, and `SCHEMA_VERSION` as a public stability surface — bump `SCHEMA_VERSION` to change shape. (Telemetry records are versioned independently — `telemetry::SCHEMA_VERSION = 3`.)

**Execution model.** One `run` invocation per matching file, **sequential** (rayon parallel dispatch is a possible follow-up). Per-gate wall-clock is `HECTOR_TIMEOUT` env (secs) → `execution.timeout_secs` (default 30), clamped ≥1. No sandboxing in 0.3 — the timeout is the only execution rail.

**Scope matching** (`config/scope.rs`) deliberately diverges from raw globset: bare patterns without `/` also register as `**/<pattern>`, so `*.py` matches at any depth — mirrors bully's semantics. Don't "fix" it. Applies to each gate's `files` list.

## Conventions

- A gate is exactly two fields: `files` (glob or list) + `run` (a shell string, handed to `sh -c` verbatim). There are no engines, no `severity`, no output-parsing modes — a gate blocks by exiting `2` and owns its own message. Don't reintroduce per-rule kinds.
- Test fixtures live in `tests/fixtures/` at the repo root; crate tests use relative paths.
- `Cargo.lock` is gitignored (workspace policy) — do not commit.
- Binary is `hector`, not `hector-cli`.
- Trust enforcement lives in the CLI `check` command (`commands/check.rs`), not in `HectorEngine::load`. Read-only commands (`validate`, `explain`, `show-resolved-config`, `doctor`) do not enforce trust. `doctor` is intentionally minimal until Plan 3.
