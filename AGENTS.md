# AGENTS.md

Guidance for AI coding agents working in this repo.

## What this is

Rust rewrite of [dynamik-dev/bully](https://github.com/dynamik-dev/bully) — policy-enforcement pipeline for AI coding agents. Status: **0.1 complete**. Two engines (`script`, `ast`) wired — hector is a static gate; LLM evaluation was removed in 0.2. CLI ships `check`, `trust`, `validate`, `init`, `migrate`, `baseline`. Claude Code adapter at `adapters/claude-code/` shipped (0.1c). Plan 0.2 adds OpenAI + Aider + pre-commit. Authoritative docs: `specs/overview.md` (1.0 vision), `specs/2026-05-11-hector-plan-and-0.1-design.md` (phasing + 0.1 design); per-phase plans in `plans/`.

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
cargo test --test cli_e2e_script_rules      # single integration test file
cargo test <name>                           # filter by test-fn name
cargo clippy --all-targets -- -D warnings   # lint
cargo fmt
bash scripts/ci-coverage.sh                 # per-file ≥80% region-coverage gate (matches CI)
```

Snapshot tests: `insta` — `cargo insta review` after intentional verdict-shape changes. CLI tests use `assert_cmd` against the compiled binary.

## Architecture

Cargo workspace, two crates:

- **`hector-core`** — library. Modules:
  - `config` — parse v1/v2 YAML, glob scope matching, `extends:` resolution
  - `diff` — unified-diff parser
  - `engine` — `RuleEngine` trait + two impls: `script` (capability-sandboxed exec) and `ast` (via `ast-grep-core`)
  - `runner` — orchestrates: load → trust-verify → scope-match → dispatch engine → baseline-filter → telemetry-log
  - `trust` — canonical-YAML sha256 fingerprint
  - `verdict` — Pass/Warn/Block + locked JSON shape
  - `disable` — `hector-disable: <rule-id>` line directives (one rule per directive; directive ends at whitespace/`*`/`/`)
  - `baseline` — record-and-filter existing violations by `rule_id::file::line` fingerprint
  - `telemetry` — `.hector/log.jsonl`, append-only check log
- **`hector-cli`** — thin binary, name `hector`. `cli.rs` defines clap subcommands; `commands/{check,trust,validate,init,migrate,baseline}.rs` are one-function adapters into core.

Three load-time invariants enforced by `HectorEngine::load` (`crates/hector-core/src/runner.rs`):

1. **Trust gate.** `trust::verify` recomputes the sha256 of the YAML with `trust:` stripped and keys sorted; mismatch errors (CLI exit 1). Only defense against malicious `script:` rules — capabilities are accident-protection, not adversarial-protection. See `docs/security/trust.md` and `docs/security/capabilities.md`.
2. **Schema version.** `parser::SUPPORTED_SCHEMAS = [1, 2]`. v1 is legacy bully; `is_legacy()` is the migration hook.
3. **Extends.** `config::extends::resolve` does a cycle-detected DFS; inherited rules fill gaps but **local rules win on collision**, and `trust:` is never inherited.

**Exit-code contract** (`commands/check.rs`):

- `0` — Pass or Warn
- `1` — internal/config error (untrusted, parse failure, missing file)
- `2` — Block (≥1 error-severity policy violation)
- `3` — InternalError (≥1 engine runtime error; `__internal` violations present — e.g. AST refused diff, script spawn failure)

Adapters fail-open on exit 3 by default; opt-in fail-closed via `HECTOR_FAIL_CLOSED_ON_INTERNAL=1`.

Consumed by CI and editor adapters — do not break.

**Verdict JSON** (`verdict.rs`): "locked-but-unstable" at 0.1, freezes at 0.3. Treat `Verdict`, `Violation`, `Status`, `Severity`, `Engine`, `SCHEMA_VERSION` as a public stability surface now — bump `SCHEMA_VERSION` to change shape.

**Capability sandbox** (`engine/capability.rs`): **Linux-strict for network, advisory for writes**. On Linux, `network: false` unshares the net namespace when privileged (best-effort with warning otherwise). Writes policy: no-op pending CAP_SYS_ADMIN-via-CLONE_NEWUSER work in 0.2. On macOS, all constraints are advisory; the command runs unrestricted.

**Scope matching** (`config/scope.rs`) deliberately diverges from raw globset: bare patterns without `/` also register as `**/<pattern>`, so `*.py` matches at any depth — mirrors bully's semantics. Don't "fix" it.

## Conventions

- New engines plug in via the `EngineKind` enum (`script`, `ast`) + a match arm in `runner::run_engine`. `EngineKind` is matched exhaustively (no wildcard arm), so adding a variant fails to compile until every site handles it. Configs naming any other engine — including the removed `semantic`/`session` — are rejected by serde at parse time as an unknown variant. Don't smuggle new logic into an existing arm.
- Test fixtures live in `tests/fixtures/` at the repo root; crate tests use relative paths.
- `Cargo.lock` is gitignored (workspace policy) — do not commit.
- Binary is `hector`, not `hector-cli`.
- `Engine::Trust` in the verdict enum is the catch-all for _internal_ engine errors and trust-gate failures (semantic mismatch acknowledged); rename is on the table before the verdict shape freezes at 0.3.
