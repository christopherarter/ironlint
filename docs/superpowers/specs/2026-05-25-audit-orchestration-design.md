# Hector `check` audit remediation — orchestration design

**Date:** 2026-05-25
**Author:** Claude (Opus 4.7) via `superpowers:brainstorming`
**Source audit:** [`docs/audits/2026-05-24-check-end-to-end-audit.md`](../../audits/2026-05-24-check-end-to-end-audit.md) — 21 findings (2 P0, 7 P1, 6 P2, 6 P3).
**Next step:** This design feeds `superpowers:writing-plans` to produce `plans/2026-05-25-audit-remediation.md`; that plan is dispatched via `superpowers:subagent-driven-development`.

## Scope

This doc is the orchestration *strategy* — sub-agent roles, phase batching, sequencing, quality gates, wire-format coordination. It is **not** the per-finding fix detail (the audit owns that) and **not** the executable plan (writing-plans produces that next).

Out of scope: re-deciding any audit finding's fix-the-right-way recommendation, expanding 0.2 to other roadmap items beyond the audit, redesigning the verdict / deferred-envelope wire format beyond the changes the audit already specifies.

## Goal

Land fixes for all 21 audit findings such that:

1. **P0s ship first** as small focused commits; nothing else depends on their landing schedule.
2. **The four wire-format-touching changes ship as one coordinated 0.2 release** with a single CHANGELOG migration section, per audit guidance (B7 `Status::InternalError`; deferred envelope v3 = B4 + B5 + C5; verdict version policy C6; trust fingerprint C1).
3. **Every fix lands with a failing test first** — CLAUDE.md rule. The failing test becomes the regression coverage.
4. **Every phase clears a code review by a separate agent** — CLAUDE.md rule. The reviewer's session is fresh; they see the diff and the relevant audit finding only.
5. **Per-file region coverage stays ≥80%** — verified via `scripts/ci-coverage.sh` after each phase.
6. **Two design pins (D1, D6)** are resolved up-front before any code dependent on the choice ships.

## Precedent

`plans/archive/2026-05-12-bug-audit-remediation.md` is the canonical shape for this kind of work: one plan file, phase-ordered, per-phase fan-out numbers, per-task TDD steps. This design follows that precedent.

## Approach (chosen)

**Phase-ordered single plan, subagent-per-task.** One `plans/2026-05-25-audit-remediation.md` with seven phases, dispatched via `superpowers:subagent-driven-development`. Within each phase, tasks tagged `[parallel]` or `[serial]` based on the file-touch graph. Phases serialize between each other by default; one explicit exception (Phase 3 parallel with Phase 2, see below) where file-touch sets are fully disjoint. Tasks parallelize within a phase where their file-touch sets are disjoint.

**Alternatives rejected:**

- *Per-finding micro-plans (21 plan files).* Maximum theoretical parallelism but ~half the findings overlap on `runner.rs`, `verdict.rs`, or `commands/check.rs`. Merge-conflict thrash and review burden swamp the gain. Also fragments the wire-format coordination story across many CHANGELOG entries.
- *Long-lived `0.2-audit-remediation` feature branch with sub-PRs.* Cleaner CHANGELOG but creates drift from `main`, rebase pain, and conflicts with the precedent.

## Sub-agent roles

| Role | Agent type | Responsibility |
|------|------------|----------------|
| **Implementer** | `general-purpose` (Rust skill loaded via skill stack) | One task at a time. TDD: failing test → verify red → minimal impl → verify green → commit. |
| **Code reviewer** | `general-purpose` (fresh session, no prior context) | Reads the diff plus the relevant audit finding. Approves or requests changes before phase advances. |
| **Design pinner** | `Plan` agent or orchestrator with `AskUserQuestion` | Resolves D1 (`added_lines` contract A vs B) and D6 (`extends:` first-parent vs last-parent). Phase 0 only. |
| **Wire-format steward** | Orchestrator (the controlling session) | Holds `SCHEMA_VERSION` / `DEFERRED_SCHEMA_VERSION` bumps until all coordinated changes in Phase 5 land. Owns the CHANGELOG migration entry. |
| **Coverage gatekeeper** | `Bash` invocations of `scripts/ci-coverage.sh` plus the `cleanup-build-artifacts` skill | Runs at the end of each phase. Blocks merge if any file under `crates/*/src/` drops below 80% region coverage. |

The reviewer-separation rule from CLAUDE.md is enforced *structurally*: the implementing agent's task is to land a commit; the reviewing agent is dispatched as a separate sub-agent with no shared context. The orchestrator passes the reviewer the diff range and the audit anchor only.

## Phase batching

| Phase | Findings | Fan-out | Gates | Why |
|-------|----------|---------|-------|-----|
| 0 — Design pins | D1, D6 | 1 (decisions) | None — first | D1's choice shapes runner.rs work in Phase 2; D6's choice may need a test in extends.rs. |
| 1 — P0 ship-blockers | A1, A2 | 2 (parallel) | Phase 0 | Independent files (`baseline.rs` vs `diff/parser.rs`); both ship-blockers. |
| 2 — Path/scope helpers | B1, B2, C4, D4 | 1 (serial) | Phase 1 | All four touch `runner.rs` and share `resolve_input_path` + `rule_matches_path` helpers. |
| 3 — Linux capability sandbox | B6 | 1 (serial, specialist) | Phase 1 (parallel-safe with Phase 2) | Self-contained in `engine/capability.rs`. Adds `unsafe` for `clone(2)`-per-child. Disjoint from Phase 2's `runner.rs` work. |
| 4 — Parser robustness | C2, C3, D2 | 1 (serial) | Phase 1 — same files as A2 | All in `diff/parser.rs` + `commands/check.rs`. Adds `ChangeOp` enum. |
| 5 — Wire-format v0.2 coordination | C6, B7, B4, B5, C5, B3, C1 | 2–3 within | Phases 2, 3, 4 | The audit's "coordinated 0.2 release." One PR. Schema bumps only at end-of-phase. |
| 6 — Standalone wins | D3, D5 | 2 (parallel) | None | Two small disjoint fixes. |

**Phase 0** is decisions, not code. Two `AskUserQuestion` prompts drive design pins. Required because:
- D1 ("`added_lines` contract A or B") determines whether AST/parsed-script violations get filtered to added lines and whether passthrough scripts need pre-content re-run. Affects what Phase 2 looks like.
- D6 (`extends:` precedence first-parent-vs-last-parent) determines whether the merge order in `config::extends` needs to flip or just gets documented. Affects what Phase 6 looks like (D6 lives there but the test shape changes).

**Phase 3 parallel-safe with Phase 2 (the one cross-phase exception):** B6 lives entirely in `engine/capability.rs`; the path/scope work in Phase 2 lives in `runner.rs`. The orchestrator MAY dispatch them simultaneously after Phase 1, or run serially if reviewer bandwidth is the bottleneck. All other phases stay strictly serial.

**Phase 5 ships as a single PR**, contrary to the per-commit-to-main default. Rationale: it's contract-shaped (every adapter and consumer in the field sees the new schema versions and trust fingerprint at once), and the audit explicitly asks for one CHANGELOG migration section. The PR is reviewed sub-task by sub-task internally; merge happens once when the phase closes.

## Wire-format coordination (Phase 5 internal order)

Sub-task sequence inside Phase 5:

1. **C6 first** — pin schema-version policy (additive-no-bump vs `major.minor`). Document in `verdict.rs` doc comment and `docs/telemetry.md`. Add `version_only_bumps_on_breaking_change` invariant test. **Every later task in this phase follows this rule.**
2. **B7** — `Status::InternalError` variant + exit code 3. Adapters (Claude Code hook + opencode plugin) gain an exit-3 handler; default = allow with stderr message; strict CI opt-in via `HECTOR_FAIL_CLOSED_ON_INTERNAL=1`.
3. **B4 + B5 + C5 together** — these all touch `DeferredPayload` / `build_evaluator_input` / `crate::llm::prompt`. One agent, one set of commits, one `DEFERRED_SCHEMA_VERSION` bump at the end.
4. **B3** — subagent-session stop path. Reuses the new envelope shape from step 3. Adds session-aggregate framing.
5. **C1** — trust fingerprint canonicalization through `serde_json::Value`. One-time re-trust nudge in the load-error message. Re-trust every checked-in `.hector.yml` in the same commit.
6. **Final commit (phase-closing)** — CHANGELOG migration section, snapshot updates, version bumps for the affected crates, one-line release-note draft. Single squash-merge or as-is merge per project preference.

## Per-task contract

Every task in the generated plan follows the prior precedent's shape (also documented in `superpowers:writing-plans`):

```
**Files:** modify / test
- [ ] Step 1: Write the failing test
- [ ] Step 2: Run it; verify red
- [ ] Step 3: Minimal implementation
- [ ] Step 4: Run it; verify green
- [ ] Step 5: Run `cargo test --all-targets` (catches regressions, not just the new test)
- [ ] Step 6: Run `cargo fmt && cargo clippy --all-targets -- -D warnings`
- [ ] Step 7: Commit
```

Sub-agents are instructed (in the plan preamble) to invoke `superpowers:test-driven-development` and to run the project-local `cleanup-build-artifacts` skill (`/.agents/skills/cleanup-build-artifacts/SKILL.md`) on completion.

## Quality gates

Between phases (orchestrator-enforced):

1. `cargo fmt -- --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test --all-targets --all-features`
4. `bash scripts/ci-coverage.sh` — per-file ≥80% region
5. Code-review subagent approves the phase's diff range
6. If any `unsafe` was added (Phase 3): `cargo +nightly miri test` where applicable, or document the miri-exempt path

A failing gate halts the next phase. The orchestrator does not move on; instead it spawns a fix-and-re-gate sub-cycle.

## Final integration

After Phase 6, the orchestrator runs end-to-end:

- Fresh `cargo clean && cargo test --all-targets --all-features` from a clean checkout
- `scripts/ci-coverage.sh` across the workspace
- Adapter integration tests under `adapters/claude-code/tests/` and `adapters/opencode/tests/` against the new envelope shape (validates B3, B4, B5, B7 end-to-end)
- Manual smoke test: `hector check`, `hector check --diff`, `hector check --session` against a real `.hector.yml` configured for both direct-API and `claude-code-subagent` providers
- Confirm `docs/audits/2026-05-24-check-end-to-end-audit.md` has every checkbox ticked

## Risks & mitigations

- **B6 (`clone(2)`-per-child) requires more `unsafe` than is reasonable.** Mitigation: keep the audit's rejected-alternative ("document + reject mixed-network configs at load") as a fallback. Decision point lives inside Phase 3; orchestrator pauses to surface the trade if `unsafe` blast radius exceeds ~150 lines.
- **C1 (trust fingerprint) breaks every checked-in `.hector.yml` simultaneously.** Mitigation: the load-error message gains a "run `hector trust` to re-sign" nudge before C1 ships; CHANGELOG and release notes call this out as the headline 0.2 migration.
- **Phase 5 is large.** Mitigation: it ships as one PR but is reviewed sub-task by sub-task (B7 → C6 → B4+B5+C5 → B3 → C1 → close). The reviewer agent has the audit anchors as ground truth.
- **A code-review subagent has no project memory.** Mitigation: the orchestrator hands it `CLAUDE.md` + the specific audit finding by file:line. The reviewer can read the rest of the repo as needed but starts from those anchors.
- **D1 design pin choice (whole-file vs added-lines-only diff mode) is contested.** Mitigation: Phase 0's `AskUserQuestion` presents both options with their downstream cost. If the user prefers B (added-lines-only), Phase 2's path/scope work grows by one task (added-lines threading); if A, it shrinks (drop `added_lines` field).

## Deliverable

A single executable plan at `plans/2026-05-25-audit-remediation.md`, produced next by `superpowers:writing-plans` from this design. The plan's task tree is the orchestration surface; sub-agents read it, claim tasks, and tick boxes as they land.
