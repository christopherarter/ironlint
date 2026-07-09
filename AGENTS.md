# AGENTS.md

Guidance for AI coding agents working in this repo.

## What this is

Rust rewrite of [dynamik-dev/bully](https://github.com/dynamik-dev/bully) — local CI for AI coding agents. Status: **0.4 "checks pipeline" redesign merged.** A check is `files` (globs) + `run` (or `steps`) + `on` (lifecycle); ironlint matches a touched file to checks, runs each command with the ABI on env + proposed content on stdin, and reads only the exit code — **any nonzero exit (1–125) blocks**. No per-rule engines, no severity, no LLM. CLI ships `check` (with `--file`, `--diff`, or bare for a repo-wide sweep), `validate`, `init` (scaffolds `.ironlint.yml` AND onboards ironlint's hook into detected coding agents — claude-code, codex, pi, opencode), `explain`, `show-resolved-config`, `doctor` (reports per-harness adapter status in the `checks[]` array), `trust` (blesses the out-of-repo store; `check` fails closed — exit 4 — on untrusted config/checks), `gate-bash` (pure-Rust Bash-command classifier the adapters' Bash branches shell out to — blocks `ironlint trust` and Bash writes to the policy surface; not a `check`, not trust-gated, runs with no `.ironlint.yml`), `update` (self-updates the binary to the latest GitHub release via the dist install receipt). **Every adapter also gates the agent's Bash tool** via `gate-bash`, closing the self-trust escape hatch (an agent can't run `ironlint trust` or write `.ironlint.yml` / `.ironlint/scripts/` through Bash). Authoritative design: `specs/2026-06-28-ironlint-checks-pipeline-design.md`; bash-gate design: `docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`; per-phase plans in `plans/`.

**Not yet built (later plans):** `ironlint verify` + the full `doctor` expansion.

## Rules

- Bug fixes start with a failing test (use the test-writing skill). The failing test becomes regression coverage.
- After completing a coding task, request code review from a separate agent.
- Your code reviews are reviewed by the principal engineer — do deep work.
- Tool hasn't shipped; no hedging.
- Rust files under `crates/*/src/` must meet ≥80% **region** coverage (distinct decision points — branches, short-circuits, match arms — not executed lines). CI enforces per-file via `scripts/ci-coverage.sh` (cargo-llvm-cov). Code added without bringing the file to the gate breaks the build.
- Cognitive complexity per function is capped at **15** via clippy (`clippy.toml`, with `#![warn(clippy::cognitive_complexity)]` at each crate root). Refactor over annotate; reach for `#[allow(clippy::cognitive_complexity)]` only when complexity is intrinsic to the function and decomposing would scatter the flow — document why.
- Mutation testing is a **local, ad-hoc** investigative tool, not a CI gate (would burn runner minutes). `cargo install cargo-mutants` once, then point it at a file or diff: `cargo mutants --file 'crates/ironlint-core/src/<name>.rs'` for one file, or `git diff main.. > pr.diff && cargo mutants --in-diff pr.diff` for the PR. A surviving mutant means tests executed the code but didn't verify what it does — treat survivors in code you touched as a coverage gap.
- Clean up build artifacts you produced once the task is done. If you ran `cargo build --release` or built a one-off binary to verify behavior, drop it with `cargo clean -p <crate>` or `rm target/release/<bin>` after verification. Same for throwaway files like `pr.diff`, ad-hoc tarballs, scratch `cargo mutants` output, or any binary built for a single check. The persistent `target/` you're actively iterating in stays — this rule is about artifacts *this task* created, not the working tree.

## Commands

```bash
cargo build --release                       # produces ./target/release/ironlint
./target/release/ironlint check              # bare = repo-wide sweep (batched where checks allow)
cargo test                                  # all workspace tests
cargo test -p ironlint-core                 # core only
cargo test -p ironlint-cli                  # CLI only
cargo test --test cli_e2e_gates             # single integration test file (checks pipeline)
cargo test <name>                           # filter by test-fn name
cargo clippy --all-targets -- -D warnings   # lint
cargo fmt
bash scripts/ci-coverage.sh                 # per-file ≥80% region-coverage gate (matches CI)
```

CLI tests use `assert_cmd` against the compiled binary. (`insta` snapshots may exist for some surfaces — `cargo insta review` after an intentional shape change.)

## Architecture

Cargo workspace, two crates:

- **`ironlint-core`** — library. Modules:
  - `config` — parse the checks YAML (`Config { extends, execution, checks }`, `Check { files, run, steps, on, name }`, `Step { name, run }`), glob scope matching (`scope.rs`), `extends:` resolution (`extends.rs`)
  - `diff` — unified-diff parser (used by CLI `--diff` to enumerate changed files)
  - `engine` — the single check-execution model: `gate::run_gate` spawns `sh -c <run>`, feeds stdin, enforces the timeout, and classifies the exit code into a `GateOutcome` (`Pass` / `Block { message }` / `Internal(InternalReason)`). No `RuleEngine` trait, no per-engine impls.
  - `runner` — orchestrates: load → `extends`-resolve → build per-check scope matchers → dispatch per lifecycle (`write`: one invocation per matching file; `pre-commit`: one invocation for the whole matching set) → fold into a `Verdict` → telemetry-log
  - `trust` — out-of-repo allow-list at `~/.config/ironlint/trust.json` (XDG: `$XDG_CONFIG_HOME/ironlint/trust.json`). Hash covers the config bytes + every file under `.ironlint/scripts/` (sorted by relative path); keyed by the config's canonical absolute path. Atomic write on `ironlint trust`. Enforcement is at the CLI `check` layer only — `IronLintEngine::load` stays pure.
  - `verdict` — `Status` (Pass / Block / InternalError) + the locked JSON shape (`Verdict { blocks, errors, passed, .. }`, `Block`, `GateError`)
  - `disable` — `ironlint-disable: <check-id>` line directives; file-wide (a directive anywhere suppresses that check for that file). Directive ends at whitespace/`*`/`/`.
  - `telemetry` — `.ironlint/log.jsonl`, append-only check log of `PerCheckRecord`s
- **`ironlint-cli`** — thin binary, name `ironlint`. `cli.rs` defines clap subcommands; `commands/{check,validate,init,explain,show_resolved_config,doctor,trust}.rs` are one-function adapters into core.

`IronLintEngine::load` (`crates/ironlint-core/src/runner.rs`) resolves `extends` and builds the per-check scope matchers. Two things it relies on:

1. **Extends.** `config::extends::resolve` does a cycle-detected DFS; inherited checks fill gaps but **local checks win on collision**.
2. **Legacy rejection.** `config::parser` rejects any pre-0.3 config (top-level `schema_version:`, `rules:`, or `trust:`) with a curated error pointing at the checks format — there is no migration path (no install base).

(Trust is enforced at the CLI `check` layer — `check::run` calls `trust::check_trust` before invoking the engine and exits **4** on missing/mismatch (or a corrupt/unreadable trust store). A config the trust layer can't even hash — parse failure, missing `extends:` target, etc. — isn't a trust decision at all; it falls through to exit 1, the same code the subsequent `engine.load` error path uses. `IronLintEngine::load` stays pure; read-only commands do not enforce trust.)

**The check ABI** (locked stability surface — every adapter must satisfy it, every check `run` may rely on it): `$IRONLINT_FILE` (absolute path of the single file under check; not set for `pre-commit`), `$IRONLINT_FILES` (newline-joined list of all files under check; single entry for `write`, all staged files for `pre-commit`), `$IRONLINT_ROOT` (project root = the check's cwd), `$IRONLINT_EVENT` (`write`/`pre-commit`), `$IRONLINT_TMPFILE` (write-only, and only when the check's `run`/`steps` reference it: absolute path to an ironlint-materialized temp file — sibling of `$IRONLINT_FILE`, same extension — holding the proposed content; auto-removed after the check), the proposed post-edit content on **stdin** (empty for `pre-commit`). No string templating — the path travels only as an env value, never spliced into `run`.

**Check verdict contract.** The check owns the verdict via its exit code: **any nonzero exit (1–125) blocks**; `0` passes. `126`/`127`/`≥128` (signal) / wall-clock timeout → InternalError (a broken check is never a silent pass). On Block, the check's combined trimmed stdout+stderr is the message; if both are empty, the runner fills `"<check-id> blocked"`.

**Lifecycles.** `on: [write]` (default) fires per matching file on every agent write, with proposed content on stdin. `on: [pre-commit]` fires once per check before a commit — one invocation for the entire matching file set, `$IRONLINT_FILES` populated, stdin empty. `on: [write, pre-commit]` fires at both; no duplication needed (ironlint keys by check, not event).

**Exit-code contract** (`commands/check.rs`) — consumed by CI and editor adapters, do not break:

- `0` — Pass (no warning tier exists)
- `1` — config/load error (parse failure, missing file, unknown `--check`)
- `2` — Block (≥1 check exited nonzero 1–125)
- `3` — InternalError (≥1 check crashed: 127 / timeout / signal)
- `4` — Untrusted config/gates (run `ironlint trust`) — the ONE sanctioned extension of this contract (Task 3.2 / Finding C3). Emitted by the trust gate *before* the engine loads or any check runs, never from a verdict.

Adapters fail-open on exit 3 by default; opt-in fail-closed via `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`. Exit 4 is the opposite default: adapters must surface it loudly, and every pre-write adapter treats it as fail-closed — it blocks the tool call rather than allowing it through. An untrusted config must never be silently un-gated.

**Bash gate (separate built-in).** `ironlint gate-bash` (in `crates/ironlint-bash-gate`, exposed via `commands/gate_bash.rs`) is NOT a `check` and NOT trust-gated — it runs even with no `.ironlint.yml`. Its exit contract is `0` = allow / `2` = block (reason on stdout); any other exit (spawn failure, signal) the adapters treat as fail-closed. Every adapter's Bash branch shells out to it with the command on stdin; the branch runs *before* the config-existence check so it fires even in a config-less project. It blocks `ironlint trust` and Bash writes to `.ironlint.yml` / `.ironlint/scripts/` (redirects, `tee`, `sed -i`, `ed`, `cp`/`mv` onto the policy surface). Variable-substitution indirection (`iron$(echo lint) trust`) is a documented known gap — adversarial tier, out of scope. See `docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`.

**Verdict JSON** (`verdict.rs`): `SCHEMA_VERSION = 5`. Treat `Verdict`, `Block`, `GateError`, `Status`, and `SCHEMA_VERSION` as a public stability surface — bump `SCHEMA_VERSION` to change shape. (Telemetry records are versioned independently — `telemetry::SCHEMA_VERSION = 5`.)

**Execution model.** `write` dispatch is sequential: one `run` invocation per matching file. `pre-commit` dispatch runs once per check (rayon parallel across checks is a possible follow-up). Per-check wall-clock is `IRONLINT_TIMEOUT` env (secs) → `execution.timeout_secs` (default 30), clamped ≥1. No sandboxing — the timeout is the only execution rail.

**Scope matching** (`config/scope.rs`) deliberately diverges from raw globset: bare patterns without `/` also register as `**/<pattern>`, so `*.py` matches at any depth — mirrors bully's semantics. Don't "fix" it. Applies to each check's `files` list.

## Conventions

- A check is `files` (glob or list) + `run` (a shell string, handed to `sh -c` verbatim) or `steps` (a sequence of `{name, run}`), plus an optional `on` lifecycle and `name` label. There are no engines, no `severity`, no output-parsing modes — a check blocks by exiting nonzero (1–125) and owns its own message. Don't reintroduce per-rule kinds.
- Test fixtures live in `tests/fixtures/` at the repo root; crate tests use relative paths.
- `Cargo.lock` is committed (workspace policy) for reproducible release builds — cargo-dist resolves against the locked graph instead of re-resolving fresh on each runner, and users can `cargo install --locked`. CI and release builds use `--locked`; regenerate with `cargo generate-lockfile` (or a plain `cargo build`) when it drifts, then commit the update alongside the dependency bump.
- Binary is `ironlint`, not `ironlint-cli`.
- Trust enforcement lives in the CLI `check` command (`commands/check.rs`), not in `IronLintEngine::load`. Read-only commands (`validate`, `explain`, `show-resolved-config`, `doctor`) do not enforce trust. `doctor` is intentionally minimal until a later plan.
