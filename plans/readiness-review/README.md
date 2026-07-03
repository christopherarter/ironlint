# IronLint Readiness Review — Execution Plans

These are the actionable, one-task-at-a-time execution plans derived from the
2026-07-02 professional-grade readiness review of IronLint v0.7.0. Work through
them **in phase order** — each phase makes the next one safe.

> **You (the executor) are expected to be a capable-but-not-frontier model.**
> These docs are written to be executed literally. Do exactly what a task says.
> When a task tells you to *search for an anchor string*, do that instead of
> trusting a line number — line numbers drift, the anchor strings do not. If
> reality does not match what a task describes, **stop and report the mismatch**
> rather than guessing.

## The theme in one sentence

The engine is solid; the problem is that the gate **fails silent, not loud** —
in several places IronLint stops enforcing without telling anyone. Every task
below moves one such case from "silent bypass" toward "loud, visible failure,"
or closes a correctness / supply-chain / adoption gap.

## Phases

| Phase | Goal | File | Tasks |
|-------|------|------|-------|
| 1 | Stop the silent bypasses (fail loud) — small, high-value | [`phase-1-stop-silent-bypass.md`](phase-1-stop-silent-bypass.md) | 7 |
| 2 | Process & trust hardening (unblocks safe parallelism) | [`phase-2-process-and-trust-hardening.md`](phase-2-process-and-trust-hardening.md) | 7 |
| 2R | Phase-2 review follow-ups (from the post-phase-2 code review) | [`phase-2-review-followups.md`](phase-2-review-followups.md) | 12 |
| 3 | Enforcement integrity & supply chain | [`phase-3-enforcement-integrity-and-supply-chain.md`](phase-3-enforcement-integrity-and-supply-chain.md) | 10 |
| 4 | Close the promise (Cursor, CI round-trip, distribution) | [`phase-4-close-the-promise.md`](phase-4-close-the-promise.md) | 7 |
| 5 | Docs & remaining polish (backlog) | [`phase-5-docs-and-polish.md`](phase-5-docs-and-polish.md) | 33 |

Do **not** start Phase N+1 tasks that depend on Phase N until the dependency is
done. Dependencies are marked per task as **Depends on:**. Tasks with
**Depends on: none** inside a phase can be done in any order (or in parallel by
separate sessions).

## How to execute ONE task (the loop)

Every task follows the same shape. Follow it exactly:

1. **Read the whole task first.** Note its `Files` and `Depends on`.
2. **Confirm the location.** Open each listed file and search for the given
   *anchor* (a symbol name or exact string). Work from what you find, not from
   the line number in the doc.
3. **Write the failing test first** (repo rule — see below). Run it and confirm
   it **fails for the stated reason**. If it passes already, the bug may be
   fixed — stop and report.
4. **Make the change** exactly as described.
5. **Run Standard Verification** (below). Everything must pass.
6. **Check the box** for the task in this README's checklist and in the phase
   file, and stop. One task per sitting is fine — that is the point.

## Repo rules you MUST honor on every code task

These come from `AGENTS.md` (the authoritative contributor guide). They are not
optional:

- **Bug fixes start with a failing test.** Write the test, watch it fail, then
  fix. That failing test becomes the regression guard. (Doc-only tasks and
  config/CI tasks are exempt — they say so.)
- **≥80% region coverage per file.** New code must not drop a file below the
  gate. CI enforces this via `scripts/ci-coverage.sh`. *(Note: that script needs
  `cargo-llvm-cov` + `llvm-tools-preview`, which may not be installed locally —
  if `bash scripts/ci-coverage.sh` errors about missing tooling, that gate runs
  in CI instead; still make sure every new branch you add has a test.)*
- **Cognitive complexity ≤ 15 per function** (clippy enforces it). If a function
  gets complex, **extract helpers** — do not add `#[allow(...)]` unless the
  complexity is intrinsic and you document why.
- **Clean up build artifacts you created.** If you ran `cargo build --release`,
  `cargo mutants`, or built a one-off binary to verify, remove it afterward
  (`cargo clean -p <crate>` / `rm` the stray file). Do not delete the shared
  `target/` you are iterating in.
- **After finishing a task, request a code review from a separate agent** before
  considering it merged. (If you cannot spawn one, leave the change on a branch
  and note that review is pending.)
- **`Cargo.lock` is currently gitignored** — but Task 3.5 changes that. Until
  3.5 lands, do not commit `Cargo.lock`.
- **Binary name is `ironlint`** (crate `ironlint-cli`). **Config file is
  `.ironlint.yml`.** **ABI env vars are `IRONLINT_*`.** Schemas are locked at
  version 5 — do not bump `SCHEMA_VERSION` unless a task explicitly says to.

## Standard Verification (run before marking any code task done)

From the repo root, in this order. Each must exit 0:

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
```

For a change scoped to one crate you may run `cargo test -p ironlint-core` or
`-p ironlint-cli` while iterating, but run the full `--workspace` before done.

If you changed an adapter shell/TS script, also run that adapter's own tests
where they exist:

```bash
# opencode / pi adapters have their own suites:
cd adapters/opencode && bun test        # if bun is installed
cd adapters/pi && npm test              # if node is installed
```

If you touched anything behind an `insta` snapshot, review it:

```bash
cargo insta review     # accept only intentional snapshot changes
```

## Locked stability surfaces — do not break these without a task telling you to

A regression here breaks every downstream adapter/CI consumer. If a task seems
to require changing one of these and does not say so explicitly, **stop and
report**:

- The **check ABI**: `$IRONLINT_FILE`, `$IRONLINT_FILES`, `$IRONLINT_ROOT`,
  `$IRONLINT_EVENT`, `$IRONLINT_TMPFILE`, and proposed content on **stdin**.
- The **exit-code contract** for `ironlint check`: `0` pass · `1` config/load
  error · `2` block · `3` internal error. *(Phase 3 Task 3.2 deliberately adds
  `4` for untrusted — that is the one sanctioned extension, done carefully.)*
- The **verdict JSON** shape and `SCHEMA_VERSION = 5`.

## Progress checklist

### Phase 1 — Stop the silent bypasses
- [ ] 1.1 Add the missing `LICENSE` file
- [ ] 1.2 `init --dry-run` must not write or bless anything
- [ ] 1.3 Usage errors must exit 1, not 2
- [ ] 1.4 Windows: fail loud when `sh` is unavailable
- [ ] 1.5 Reasonix adapter: honor `IRONLINT_FAIL_CLOSED_ON_INTERNAL`
- [ ] 1.6 `doctor`: add a trust row and a "no hooks wired" row
- [ ] 1.7 Fix the Claude Code "before it lands on disk" doc claim

### Phase 2 — Process & trust hardening
- [x] 2.1 Kill the whole process tree on timeout
- [x] 2.2 Lock the trust store; unique temp file; recover from corruption
- [x] 2.3 Trust hash walk: do not follow symlinks / read non-regular files
- [x] 2.4 Reject duplicate check ids at parse time
- [x] 2.5 Sweep stale temp files; fix the false cleanup comment
- [x] 2.6 OpenCode adapter: stop shadow-writing the real file
- [x] 2.7 `extends`: inherit `execution` (timeouts), not just checks

### Phase 2 review follow-ups — from the post-phase-2 code review
(Details, severities, and verification provenance in
[`phase-2-review-followups.md`](phase-2-review-followups.md). 2.8 and 2.9 are
blockers; do them before starting Phase 3.)
- [ ] 2.8 OpenCode adapter: harden the spawn/exit translation (no throw on missing binary; `exitCode === null` → internal tier; async spawn)
- [ ] 2.9 `run_gate`: normal path must not hang past the timeout (+ f65d68c regression tests)
- [ ] 2.10 Forward Ctrl-C/SIGTERM to running check groups
- [ ] 2.11 Trust store: never silently destroy entries on parse failure
- [ ] 2.12 Guard config-file reads like the gates walk (FIFO hang)
- [ ] 2.13 Break the gates-dir tmpfile ↔ trust-hash deadlock
- [ ] 2.14 `execution` merge: field-level, not block-level (`execution: {}`)
- [ ] 2.15 Clamp the tmpfile sweep age against the resolved timeout
- [ ] 2.16 Reclaim orphaned `trust.json.tmp.*` files
- [ ] 2.17 Duplicate-key guard: precise message, non-degradable mechanism
- [ ] 2.18 Move the config-root sweep out of `IronLintEngine::load`
- [ ] 2.19 OpenCode edit path: honest comment, visible skip

### Phase 3 — Enforcement integrity & supply chain
- [ ] 3.1 Migrate Claude Code adapter to `PreToolUse` (deny before write)
- [ ] 3.2 Add exit code 4 for untrusted; make all adapters surface it loudly
- [ ] 3.3 Hash in-repo scripts referenced by `run:` (close the RCE gap)
- [ ] 3.4 Run checks with a scrubbed environment (no secret inheritance)
- [ ] 3.5 Commit `Cargo.lock`; reverse the gitignore policy
- [ ] 3.6 Pin all GitHub Actions to commit SHAs
- [ ] 3.7 Add `cargo-deny`, an MSRV, and a scheduled CI run
- [ ] 3.8 Pin the five unpinned contract arms with tests
- [ ] 3.9 Real contract tests for the claude-code + reasonix hooks
- [ ] 3.10 One unreadable file must not abort a whole `--diff` batch

### Phase 4 — Close the promise
- [ ] 4.1 Build the Cursor adapter
- [ ] 4.2 Ship a GitHub Action (CI round-trip)
- [ ] 4.3 `init` installs a chain-safe git `pre-commit` hook; add `check --staged`
- [ ] 4.4 pre-commit checks read staged content, not the worktree
- [ ] 4.5 Distribution: Homebrew tap, crates.io, npm wrapper
- [ ] 4.6 Parallelize check dispatch; add a verdict cache
- [ ] 4.7 `extends` from git URLs (shareable check packs)

### Phase 5 — Docs & remaining polish
See [`phase-5-docs-and-polish.md`](phase-5-docs-and-polish.md) for its own
30-item checklist.

## Provenance

Full findings, severities, and the reasoning behind the sequencing live in the
published review report (a navigable page). Finding IDs referenced in these
tasks (e.g. `C3`, `R5`, `E2`) map to that report. This folder is the executable
form; the report is the argument.
