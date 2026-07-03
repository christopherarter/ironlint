# Spec-Fix Instructions: Determinism Eval Bench

**Target file:** `specs/2026-07-02-determinism-eval-bench-design.md`
**Scope:** Revise the spec only. Do NOT build the harness, create `evals/`, or write any Rust/Node code. Every change below is an edit to the spec document. When all items are done, the spec should survive a hostile methodology review.

## Context you need

IronLint is a deterministic static gate for AI coding agents (Rust, this repo). The spec designs a benchmark proving the thesis: *an enforced deterministic gate produces fewer quality violations in landed code than prompt-based self-review, at equal budget, with no loss in task success.* Three arms: A (baseline, no feedback), B (complete rule text in prompt + post-completion self-review rounds), C (real `ironlint check` gates every write pre-landing; blocked writes never land).

A design review found two blockers that make the headline claim unfalsifiable, one mechanical flaw in the flagship gate check, and several methodological gaps. Fix all of them. Items are ordered by severity; section names are the stable reference (line numbers are from the current version and will shift as you edit).

---

## Blocker 1 — Task success is unmeasured; doing nothing scores perfectly

**Problem.** The Testing section requires each fixture's `node:test` suite to pass on the *unmodified* fixture, and the guardrail metric is pass rate on final code. Together those mean "functional success" only detects regressions — it never measures whether the task was accomplished. A C run classified `blocked-incomplete` leaves the fixture untouched and therefore scores zero violations AND 100% test pass: the arm the bench exists to vindicate can win by inaction. "No loss in functional task success" is currently unfalsifiable.

**Required change.** Give every task fixture **two suites**:

- **Baseline suite** (`test/baseline/`): passes on the unmodified fixture; detects regressions.
- **Acceptance suite** (`test/acceptance/`): covers the new functionality the task asks for; **must fail on the unmodified fixture** (add this as an authoring invariant in the Testing section, alongside the existing baseline-must-pass invariant).

Acceptance tests are **visible to the model** — referenced in `TASK.md` and included in whatever fixture-source context the model receives (see Blocker 2) — identical across all three arms.

Update the Metrics section: **task success (acceptance-suite pass on final code) is a co-primary outcome**, not a guardrail. The baseline suite remains the regression guardrail. Update the directory layout in D2 (`test/` → `test/baseline/` + `test/acceptance/`), the Task corpus prose, and the run-classification discussion so `blocked-incomplete` and untouched-fixture runs are explicitly scored as **task failures** (acceptance suite fails), whatever their violation count.

**Acceptance check:** after your edit, a run that lands zero writes must score as a failed task somewhere in the primary results table. If it doesn't, the fix is incomplete.

## Blocker 2 — The model cannot see the fixture code

**Problem.** D4 gives the agent exactly two tools: `write_file` and `done()`. Every duplication-pillar task is built on "a helper already exists" — but a model that cannot read `src/` cannot reuse a helper it has never seen. Duplication becomes inevitable in all arms and the Pillar-1 result is meaningless. The spec never says how fixture source reaches the model.

**Required change.** Amend D4: the initial user message embeds the **complete fixture source** (every file under `src/` and `test/`, with relative paths, plus `TASK.md`), rendered by a pinned template. The rendered prompt is **hashed into the run manifest** (extend the manifest field list in the Run protocol section) and is byte-identical across arms for a given task. State explicitly that no `read_file`/`list_files` tools are added — full embedding keeps the arms simpler and the context identical.

Fixtures must therefore stay small enough to embed whole; add a sentence to the Task corpus section capping fixture size (e.g., total fixture source must fit comfortably in-context — a few hundred lines).

## Blocker 3 — Cross-file gate checks are broken under the pre-write posture

**Problem.** Arm C gates pre-write (Gate posture section): when the check runs, the proposed content is NOT on disk — the old file still is. The Pillar-1 check "run jscpd over `src/`" therefore either misses the proposed content entirely, or — if the check references `$IRONLINT_TMPFILE`, which ironlint materializes as a *sibling file in the same directory with the same extension* — jscpd scans both the old file and the tmpfile and flags every unchanged line as duplication of itself. Either way the flagship check is wrong. dependency-cruiser has a milder version: path-keyed layer rules must match the tmpfile's path, and the tmpfile has a generated name.

**Required change.** Add a subsection to the Task corpus section titled **"Authoring cross-file checks under pre-write"** specifying:

- **jscpd (duplication):** the check script builds a shadow overlay — copy `src/` to a temp dir, write the proposed content (stdin) at `$IRONLINT_FILE`'s path relative to `$IRONLINT_ROOT`, run jscpd against the overlay, compare against the committed baseline, clean up. The baseline is computed by the same script over the pristine fixture so numerator and denominator match.
- **dependency-cruiser (architecture):** layer rules must be written as **directory globs** (e.g., `src/routes/**`), never exact filenames, so they match `$IRONLINT_TMPFILE` (a sibling in the same directory). Note that pre-write depcruise can only validate the proposed file's *own* imports — that is sufficient for the "routes must not import db" rule family, and say so.
- **Biome (complexity):** per-file on stdin/tmpfile content; no cross-file issue. State this so the asymmetry is documented.

Also update the Pillar table's jscpd row so it no longer reads as a naive "jscpd over src/".

## Methodology 4 — The isolation claim is overstated

**Problem.** The Experimental design section claims "the only explanatory variable is the *source* of the feedback." False: C differs from B in three bundled ways — verdict source (deterministic vs. self), timing/granularity (inline per-write vs. post-hoc batch), and enforcement (blocked writes never land).

**Required change.** Rewrite that sentence to claim the **bundle**: C isolates *the enforced deterministic gate as a unit* (the thing IronLint actually ships), and name the three bundled differences honestly. In Non-goals, replace the proposed 4th arm ("generic self-review without rule text" — a weaker variant no reviewer will request) with the arm reviewers WILL request: **B′ — inline self-review**, where after each `write_file` the model is prompted to check that file against the rules before it lands. B′ matches C's timing and enforcement point and differs only in verdict source. Keep it future work, but name it as the designed answer to the decomposition critique.

## Methodology 5 — The primary metric is near-tautological for arm C

**Problem.** Landed code in C passed the exact measurement tools by construction; a low violation count is guaranteed, not discovered.

**Required change.** Two edits to the Metrics section:

1. Make the headline a **joint outcome**: violations in landed code AND acceptance-suite pass AND completion within T/R, presented per-arm in one table. C wins only if it holds all three.
2. Add a mechanism metric, computable from transcripts at zero extra cost: **first-attempt violation rate** — for arm C, the fraction of `write_file` attempts the gate blocked (and the violation counts in those blocked writes). This is the direct evidence for the arXiv mechanism (the model generates slop it cannot self-detect but fixes on signal) and belongs in the headline write-up. Acknowledge the near-tautology explicitly in the spec — naming it defuses it.

## Methodology 6 — "No loss in functional success" needs non-inferiority

**Problem.** With 12 tasks as the unit of analysis, a non-significant difference is weak evidence of "no loss." Absence of significance ≠ equivalence.

**Required change.** In the Statistics paragraph, pre-register a **non-inferiority margin** for task success: C is declared "no loss" only if the 95% CI of the C−B acceptance-pass-rate difference excludes a drop worse than **10 percentage points** (use this default unless you have a principled better one; the point is the margin is fixed in the spec before any data exists).

## Methodology 7 — Pin the unit of analysis

**Required change.** In the Statistics paragraph, state explicitly: reps are **averaged within task first**; the sample is **n = 12 task-level paired differences** per arm-pair per model. All CIs and the Wilcoxon test operate on task-level values. (Prevents pseudo-replication over 60 reps.)

## Implementation 8 — Trust must be blessed per throwaway copy

**Problem.** D5 says setup blesses each fixture's config via `ironlint trust`, but ironlint's trust store is keyed by the config's **canonical absolute path**, and D4 runs every rep in a throwaway fixture copy at a fresh path. Blessing the checked-in fixture does nothing for the copies.

**Required change.** Amend D5: `ironlint trust` runs **per run-copy during run setup**, after the copy is created. Note the consequences: trust-store churn over thousands of runs (harmless but real) and, if runs are ever parallelized, contention on the trust file's atomic writes — serialize the bless step.

## Implementation 9 — Scoring executes model-written code

**Problem.** `node --test` on final code is arbitrary code execution of untrusted model output on the host, with `BENCH_API_KEY` in the environment.

**Required change.** Add to Error handling (or a new Security note): the scoring step (tests + measurement tools over final code) runs in a subprocess with a **scrubbed environment** (no `BENCH_API_KEY`, minimal PATH) and, ideally, no network. Recommend a container for published runs. Cheap to specify, embarrassing to omit from a published artifact.

## Implementation 10 — R means different things per arm

**Problem.** A B feedback round is a full review pass over all rules; a C feedback round is one blocked write. Calling both "R" makes the parity claim mushy.

**Required change.** In the Budget parity paragraph, acknowledge the semantic difference and require the report to show **blocked-write counts (C) and review-round counts (B) side by side**, alongside the existing per-arm token totals, rather than presenting a single "R used" column.

## Implementation 11 — api-error exclusion can bias by arm

**Problem.** Arm C makes more API calls per run (gate-feedback turns), so it has more exposure to provider errors; excluding `api-error` runs from aggregates could selectively prune C.

**Required change.** In Error handling, require api-error exclusions to be **reported per arm** (not just globally), so readers can check the exclusion rate is arm-balanced.

---

## Definition of done

- [ ] All 11 items applied to `specs/2026-07-02-determinism-eval-bench-design.md`; no code written.
- [ ] Grep check: the spec no longer contains "the only explanatory variable is the *source* of the feedback" verbatim (item 4).
- [ ] The D2 directory layout shows `test/baseline/` and `test/acceptance/` (item 1).
- [ ] The manifest field list includes the rendered-prompt hash (item 2).
- [ ] A "cross-file checks under pre-write" subsection exists and mentions `$IRONLINT_TMPFILE` explicitly (item 3).
- [ ] The Statistics paragraph contains the words "non-inferiority" and "task-level" (items 6–7).
- [ ] Internal consistency pass: run classifications, metrics, directory layout, and manifest fields all agree with each other after the edits — no section still describes the old single-suite / naive-jscpd design.
- [ ] Keep the spec's existing voice and structure; edit sections in place rather than appending a changelog.
