# AGENTS.md

This file provides guidance to Codex (Codex.ai/code) when working with code in this repository.

## What this is

Hector is a Rust rewrite of [dynamik-dev/bully](https://github.com/dynamik-dev/bully) — a policy-enforcement pipeline for AI coding agents. Status: **0.1 complete**. All four engines (`script`, `ast`, `semantic`, `session`) are wired. CLI ships `check`, `trust`, `validate`, `init`, `migrate`, `baseline`, `session record`. The Codex adapter under `adapters/Codex/` is shipped (0.1c). Plan 0.2 adds OpenAI + Aider + pre-commit. The authoritative docs are `specs/overview.md` (Hector at 1.0) and `specs/2026-05-11-hector-plan-and-0.1-design.md` (phasing + 0.1 design); the per-phase plans live in `plans/`.

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
```

Snapshot tests use `insta` — review with `cargo insta review` after intentional verdict-shape changes. CLI tests use `assert_cmd` and shell out to the compiled binary. LLM HTTP paths are exercised with `wiremock` (see `crates/hector-core/tests/anthropic.rs`).

## Architecture

Cargo workspace, two crates:

- **`hector-core`** — library. Modules:
  - `config` — parse v1/v2 YAML, glob scope matching, `extends:` resolution
  - `diff` — unified-diff parser
  - `engine` — `RuleEngine` trait + four impls: `script` (capability-sandboxed exec), `ast` (via `ast-grep-core`), `semantic` (via `LlmClient`), `session` (aggregated cross-edit checks)
  - `llm` — `LlmClient` trait + `AnthropicClient` (blocking `reqwest` with configurable `base_url` for wiremock) + `NoLlm` stub
  - `runner` — orchestrates: load → trust-verify → scope-match → dispatch engine → baseline-filter → telemetry-log
  - `trust` — canonical-YAML sha256 fingerprint
  - `verdict` — Pass/Warn/Block + locked JSON shape
  - `disable` — `hector-disable: <rule-id>` line directives (one rule per directive; the directive ends at whitespace/`*`/`/`)
  - `baseline` — record-and-filter existing violations by `rule_id::file::line` fingerprint
  - `session_state` — `.hector/session.json`, accumulated edits across an agent session
  - `telemetry` — `.hector/log.jsonl`, append-only check log
- **`hector-cli`** — thin binary named `hector`. `cli.rs` defines clap subcommands; `commands/{check,trust,validate,init,migrate,baseline,session}.rs` are one-function adapters that call into core.

Three load-time invariants enforced by `HectorEngine::load` (`crates/hector-core/src/runner.rs`):

1. **Trust gate.** `trust::verify` recomputes the sha256 of the YAML with the `trust:` block stripped and keys sorted; mismatch returns an error (the CLI maps to exit 1). This is the only defense against malicious `script:` rules — capabilities are accident-protection, not adversarial-protection. See `docs/security.md`.
2. **Schema version.** `parser::SUPPORTED_SCHEMAS = [1, 2]`. v1 is legacy bully; `is_legacy()` is the migration hook.
3. **Extends.** `config::extends::resolve` does a cycle-detected DFS; inherited rules fill gaps but **local rules win on collision**, and `trust:` is never inherited.

**Exit-code contract** (`commands/check.rs`):

- `0` — Pass or Warn
- `1` — internal/config error (untrusted, parse failure, missing file)
- `2` — Block (≥1 error-severity violation)

This contract is consumed by CI and editor adapters — do not break it.

**Verdict JSON** (`verdict.rs`) is "locked-but-unstable" at 0.1 and freezes at 0.3. Treat `Verdict`, `Violation`, `Status`, `Severity`, `Engine`, and `SCHEMA_VERSION` as a public stability surface even now — bump `SCHEMA_VERSION` if you must change shape.

**Capability sandbox** (`engine/capability.rs`) is **Linux-strict, macOS-advisory**. On Linux, `network: false` uses `CLONE_NEWNET` and writes policies use `CLONE_NEWNS`. On macOS the constraints are logged and the command runs unrestricted. Do not treat capability tests as security tests on macOS.

**Scope matching** (`config/scope.rs`) deliberately diverges from raw globset: bare patterns without `/` are also registered as `**/<pattern>` so `*.py` matches at any depth — this mirrors bully's semantics. Don't "fix" it.

**LLM injection.** `Semantic` and `Session` engines need an `LlmClient`. `HectorEngine::load` constructs no LLM (semantic/session rules will then error at evaluation). Tests and library callers inject a fake or real client via `HectorEngine::builder().with_llm(Box::new(...))`.

## Conventions

- New engines plug in via the `EngineKind` enum and a match arm in `runner::check`. All four arms route to real engines today; don't smuggle new logic into an existing arm.
- Test fixtures live in `tests/fixtures/` at the repo root; crate-level tests reference them via relative paths.
- `Cargo.lock` is gitignored (workspace policy in `.gitignore`) — do not commit it.
- The binary name is `hector`, not `hector-cli`.
- `Engine::Trust` in the verdict enum is the catch-all bucket for *internal* engine errors as well as trust-gate failures (semantic mismatch acknowledged); a rename is on the table before the verdict shape freezes at 0.3.
