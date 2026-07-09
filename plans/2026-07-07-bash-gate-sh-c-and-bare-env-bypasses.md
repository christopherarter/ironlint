# Phase 5 Security — bash-gate: `sh -c` descent + bare `VAR=val` prefix (v0.9.2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close two confirmed bash-gate bypasses found during v0.9.1 release review. Both let a lazy non-reasoning model run `ironlint trust` through its Bash tool despite the `gate-bash` matcher:

1. **`sh -c 'ironlint trust'`** (and `bash -c 'ironlint trust'`) — the matcher's `normalize` step strips quotes from the *entire* string, so the gate analyzes `sh -c ironlint trust` (where sh runs only `ironlint`, `trust` becomes `$0`) instead of the quoted form the shell actually executes (where sh runs `ironlint trust` as one command string). The literal `ironlint trust` is right there in the command, but `strip_wrappers` does not descend into `sh -c`/`bash -c`'s command argument.
2. **Bare `VAR=val ironlint trust`** — the `env VAR=val ironlint trust` form IS caught (`env` is a recognized wrapper; `skip_assignments` drops `VAR=val`), but the semantically equivalent bare prefix is not — `strip_wrappers`'s fall-through `break`s on `VAR=val` before reaching the binary.

**Architecture:** Both fixes live in `crates/ironlint-bash-gate/src/lib.rs::strip_wrappers`. The matcher's contract is unchanged (`0`=allow/`2`=block/else fail-closed); these are scope extensions to the existing wrapper-stripping pattern, not new shell evaluation:

- **Task 1 (`sh -c` descent):** extend the wrapper match in `strip_wrappers` to recognize `sh` and `bash`. When the token after `sh`/`bash` is `-c`, the *next* token (the command string argument) is the segment to re-check — descend into it rather than treating the `sh`/`bash` line as a non-binary command and bailing. This mirrors how `eval`/`exec` already unwrap to their argument. The unquoted `sh -c ironlint trust` form (where sh runs only `ironlint`) is safe to re-check the same way — descending only catches the *additional* `trust` token the quoted form would have run.
- **Task 2 (bare `VAR=val` prefix):** in `strip_wrappers`'s fall-through case (currently `_ => break`), recognize a leading `VAR=val` token (a `=` present but not at position 0, with the pre-`=` part a valid shell identifier — letters/digits/underscore, not starting with a digit) and skip it, reusing the existing `skip_assignments` helper. The first-token-is-binary guard in `is_ironlint_trust` then sees `ironlint` and the every-binary scan proceeds.

**Tech Stack:** Rust (workspace, edition 2021 per Cargo.toml), pure std — no new deps. `strip_wrappers`, `skip_assignments`, `is_ironlint_binary`, `is_ironlint_trust`, and the test module all already exist in `crates/ironlint-bash-gate/src/lib.rs`.

## Global Constraints

- **TDD.** Each task starts with a failing `assert_blocks(...)` test (RED), then the minimal fix (GREEN). The failing test is the regression guard.
- **≥80% region coverage per file** under `crates/*/src/` (CI-enforced via `scripts/ci-coverage.sh`). Every new branch gets a test — both the `sh -c`/`bash -c` arm and the `VAR=val` skip arm.
- **Cognitive complexity ≤15 per function** (`clippy.toml`, `#![warn(clippy::cognitive_complexity)]`). If `strip_wrappers` crosses 15 after the `sh`/`bash` arm, decompose the wrapper-recognition into a helper rather than `#[allow]`-ing.
- **Threat tier unchanged.** "Lazy non-reasoning models." Variable-substitution indirection (`iron$(echo lint) trust`, `$IRON trust`, `base64|sh`) stays a documented known gap — adversarial tier, out of scope. Do NOT add real shell evaluation to "catch more"; that crosses the tier boundary the gate is explicitly scoped to.
- **No false positives.** After each fix, re-verify that legitimate `sh -c 'echo hi'` / `sh -c 'ironlint check'` (read-only) and `FOO=bar ironlint check` still ALLOW. The gate must not block a model running a legitimate subshell or setting a real env var for a read-only command.
- **Mutation testing** (local, ad-hoc): `cargo mutants --file crates/ironlint-bash-gate/src/lib.rs` after both tasks. Zero new survivors beyond the pre-existing documented `cd_idx + 1` intrinsic one.

## Task 1 — `sh -c 'ironlint trust'` / `bash -c 'ironlint trust'`

### RED — failing tests

- [ ] Add `blocks_sh_c_ironlint_trust_single_quoted`:
  ```rust
  #[test]
  fn blocks_sh_c_ironlint_trust_single_quoted() {
      assert_blocks("sh -c 'ironlint trust'");
  }
  ```
- [ ] Add `blocks_bash_c_ironlint_trust_double_quoted`:
  ```rust
  #[test]
  fn blocks_bash_c_ironlint_trust_double_quoted() {
      assert_blocks("bash -c \"ironlint trust\"");
  }
  ```
- [ ] Add `allows_sh_c_readonly_ironlint_check` (false-positive guard):
  ```rust
  #[test]
  fn allows_sh_c_readonly_ironlint_check() {
      assert_allows("sh -c 'ironlint check'");
  }
  ```
- [ ] Run `cargo test -p ironlint-bash-gate` — the two `blocks_*` tests FAIL (RED). The `allows_*` test must PASS (if it fails, the fix is over-blocking).

### GREEN — minimal fix

- [ ] In `strip_wrappers`, add `sh` and `bash` to the recognized-wrapper match. When the token after `sh`/`bash` is `-c`, descend into the *following* token (the command-string argument) and re-run the wrapper-stripping / segment analysis on it. Mirror the `eval`/`exec` unwrap pattern.
- [ ] Run `cargo test -p ironlint-bash-gate` — all three new tests PASS (GREEN).

### Verify

- [ ] Real-binary smoke: `printf "%s" "sh -c 'ironlint trust'" | ./target/release/ironlint gate-bash` → exit 2.
- [ ] Real-binary smoke: `printf "%s" "sh -c 'ironlint check'" | ./target/release/ironlint gate-bash` → exit 0.
- [ ] No regression: existing `blocks_*` and `allows_*` tests still pass.

## Task 2 — bare `VAR=val ironlint trust`

### RED — failing tests

- [ ] Add `blocks_bare_env_prefix_ironlint_trust`:
  ```rust
  #[test]
  fn blocks_bare_env_prefix_ironlint_trust() {
      assert_blocks("IRONLINT_ROOT=/x ironlint trust");
  }
  ```
- [ ] Add `blocks_bare_env_prefix_multiple_assignments`:
  ```rust
  #[test]
  fn blocks_bare_env_prefix_multiple_assignments() {
      assert_allows("FOO=bar BAZ=qux ironlint check"); // read-only with env prefix — must still allow
  }
  ```
  Wait — that should be `assert_allows`, not `assert_blocks`. Re-check: `FOO=bar BAZ=qux ironlint check` is read-only (check), so it must ALLOW. Use it as the false-positive guard.
- [ ] Add `allows_bare_env_prefix_readonly_command` (false-positive guard, corrected):
  ```rust
  #[test]
  fn allows_bare_env_prefix_readonly_command() {
      assert_allows("RUST_LOG=debug ironlint check");
  }
  ```
- [ ] Run `cargo test -p ironlint-bash-gate` — `blocks_bare_env_prefix_ironlint_trust` FAILS (RED). The two `allows_*` tests must PASS.

### GREEN — minimal fix

- [ ] In `strip_wrappers`'s fall-through (currently `_ => break`), check whether the current token is a `VAR=val` assignment: contains `=` not at position 0, pre-`=` part is a valid shell identifier (letters/digits/underscore, not starting with a digit). If so, skip it (advance the token cursor) and continue — reusing `skip_assignments` if its signature allows, or factoring a shared `is_assignment` helper.
- [ ] Run `cargo test -p ironlint-bash-gate` — all three new tests PASS (GREEN).

### Verify

- [ ] Real-binary smoke: `printf "%s" "IRONLINT_ROOT=/x ironlint trust" | ./target/release/ironlint gate-bash` → exit 2.
- [ ] Real-binary smoke: `printf "%s" "RUST_LOG=debug ironlint check" | ./target/release/ironlint gate-bash` → exit 0.
- [ ] No regression: existing `blocks_*` and `allows_*` tests still pass.

## Task 3 — Sibling-token regression pins (reviewer recommendation)

The or-confusion fix incidentally catches `and`/newline/comma forms, but there are no tests pinning them. A future "simplification" of the every-binary loop back to first-only would silently reopen them.

- [ ] Add parametrized (or three separate `#[test]` fn) regression pins:
  ```rust
  #[test] fn blocks_ironlint_check_and_ironlint_trust() { assert_blocks("ironlint check and ironlint trust"); }
  #[test] fn blocks_ironlint_check_newline_ironlint_trust() { assert_blocks("ironlint check\nironlint trust"); }
  #[test] fn blocks_ironlint_check_comma_ironlint_trust() { assert_blocks("ironlint check, ironlint trust"); }
  ```
- [ ] All three PASS today (caught by the existing every-binary scan). They are regression pins, not RED→GREEN.

## Task 4 — Spec + CHANGELOG + release

- [ ] Update `docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md` "Known gaps" / "Direct forms caught" — move `sh -c` and bare `VAR=val` from the gap list to the caught list.
- [ ] Update `CHANGELOG.md`: add `## [0.9.2]` entry; remove the two bullet points from 0.9.1's "Known gaps" (or mark them resolved-in-0.9.2).
- [ ] Bump `Cargo.toml` workspace version `0.9.1` → `0.9.2`, regenerate `Cargo.lock`.
- [ ] Full verification: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `bash scripts/ci-coverage.sh` (then `rm -rf target/llvm-cov-target target/llvm-cov`).
- [ ] Code review (separate agent) per CLAUDE.md before tag.
- [ ] Push main + create/push annotated tag v0.9.2.

## Non-Goals

- Variable-substitution indirection (`iron$(echo lint) trust`, `$IRON trust`, `base64|sh`, `bash scripts/x.sh`). Adversarial tier, explicitly out of scope.
- Real shell evaluation / sandboxing. The gate is a static pre-filter, not a shell interpreter.
- Extending to other wrappers (`xargs`, `find -exec`, `parallel`). Out of scope unless a realistic lazy-model form is demonstrated.
