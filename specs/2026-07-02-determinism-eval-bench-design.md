# Determinism-in-the-Loop Eval Bench — Design

## Context

[The Antidote to Code Slop](https://arter.dev/blog/the-antidote-to-code-slop/) argues that agentic coding needs a deterministic counterweight to the LLM — static gates in the agent loop ("determinism-in-the-loop"). IronLint is that gate. The claim rests on three cited results:

1. **Mechanism** — [arXiv 2412.14841](https://arxiv.org/abs/2412.14841): models detect errors in their own output poorly, but fix them readily when handed a concrete signal (failing test, static-analysis report). The gate supplies the signal the model can't generate for itself.
2. **Trend** — [GitClear's 211M-line analysis](https://www.gitclear.com/ai_assistant_code_quality_2025_research): AI adds code well and reuses it poorly — refactoring fell from 25% of changes to under 10% while copy/paste climbed.
3. **Slop proxy** — [Cognitive Complexity meta-analysis](https://dl.acm.org/doi/10.1145/3382494.3410636): the first code metric empirically validated against human comprehension time (~24,000 evaluations).

The blog asserts the thesis; nothing yet *proves* it for this tool. This spec designs a reproducible benchmark that does, runnable by anyone with a Node toolchain, the ironlint binary, and one OpenAI-compatible API key (OpenRouter, umans.ai, or any compatible endpoint).

## Thesis (the headline claim the bench must support)

> At equal iteration budget — and with the model given the full rule list either way — an enforced deterministic gate (IronLint) produces significantly fewer quality violations in landed code than prompt-based self-review, with no loss in functional task success.

The design pre-rebuts the two standard critiques:

- *"You just gave the gated arm more turns."* → All arms share the same turn cap and feedback-round cap.
- *"Just put the rules in the prompt."* → The self-review arm gets the complete rule text in its system prompt — the strongest fair version of the prompt-only approach — and the claim is that it still loses.

## Experimental design

Three arms per (task × model), K repetitions each:

| Arm | System prompt | Feedback loop | What it isolates |
|---|---|---|---|
| **A — baseline** | task only | none — model writes until it calls `done()` | the slop floor |
| **B — rules-in-prompt + self-review** | task + **complete rule list** (same rules the gate enforces, rendered as prose) | after `done()`, up to **R** rounds of: "Review your changes against the rules above and fix any violations, then call done()" | the best the prompt-only approach can do |
| **C — ironlint gated** | task only (no rule text) | every `write_file` is piped through the real `ironlint check` binary; exit 2 returns the check's Block message as the tool result and the write does **not** land; up to **R** blocked-write feedbacks per run | the enforced deterministic gate as a unit — the thing IronLint actually ships |

If C beats B, the explanatory variable is the **enforced deterministic gate as a bundle**, not a single factor. C differs from B in three conflated ways, and the bench does not decompose them: (1) **verdict source** — a deterministic tool vs. the model's self-assessment; (2) **timing/granularity** — feedback inline on every write vs. post-completion batch review; (3) **enforcement** — blocked writes never land vs. advisory self-review the model may ignore. The arXiv mechanism (verdict source) is one strand of that bundle; the bench validates the shipped product as a whole, and the decomposition critique is answered by the planned B′ arm (see Non-goals), not by this v1.

**Budget parity.** One global turn cap **T = 20** tool calls per run and one feedback-round cap **R = 5** per run, identical across arms (A simply never uses feedback rounds). B's feedback arrives post-completion and C's arrives inline per-write — that asymmetry is inherent to what each design *is*, so parity is defined as: same T, same R, and actual token usage reported per arm so readers can verify neither arm got a hidden budget advantage. **A "round" is not semantically identical across arms** — a B round is one full review pass over all rules; a C round is one blocked write — so the report must **not** present a single "R used" column. Instead it shows **review-round counts (B)** and **blocked-write counts (C) side by side**, alongside the per-arm token totals, so the granularity mismatch is visible rather than hidden behind one number.

**Gate posture.** Arm C gates **pre-write**: a blocked write never lands (the reasonix/zcode `PreToolUse` posture, not claude-code's post-write). This is the cleaner experimental model — final code is exactly the set of writes that passed — and it means a run that never passes the gate lands *no* slop, which is itself the product working.

## Harness

### D1 — Purpose-built minimal loop, not Claude Code headless

Claude Code would be ecologically valid but injects a huge uncontrolled variable (its own system prompt, retries, context management), cannot use an OpenRouter/umans key, and version-drifts. The bench instead owns a minimal agent loop: the *only* difference between arms is the feedback source. A Claude Code appendix run is future work (see Non-goals).

### D2 — New `evals/` directory; Rust crate `ironlint-bench` outside `crates/`

```
evals/
  Cargo.toml            # crate: ironlint-bench, workspace member
  src/                  # harness: loop, API client, gate driver, metrics, report
  prompts/              # pinned system/user/nudge prompt templates (checked in)
  tasks/
    package.json        # pinned npm deps: jscpd, dependency-cruiser (+ committed lockfile)
    <task-id>/
      TASK.md           # the user-facing task prompt
      .ironlint.yml     # gate checks (arm C) — also the rule source rendered into arm B's prompt
      src/              # fixture code
      test/
        baseline/       # node:test — passes on the unmodified fixture; regression guardrail
        acceptance/     # node:test — covers the new functionality the task asks for; MUST fail on the unmodified fixture
  results/              # gitignored except published headline runs
  README.md             # how to run + the citable results write-up
```

The crate lives outside `crates/` deliberately: the ≥80% region-coverage CI gate globs `crates/*/src/` and bench scaffolding shouldn't be held to library-crate coverage. Workspace-wide clippy (including the cognitive-complexity cap) still applies. The npm lockfile under `evals/tasks/` **is** committed — the workspace's `Cargo.lock` gitignore policy does not extend to the eval fixtures, whose whole point is pinned reproducibility.

### D3 — OpenAI-compatible endpoint, key from env only

`--base-url` (default `https://openrouter.ai/api/v1`) + `BENCH_API_KEY` env var. umans.ai or any compatible endpoint works by swapping the base URL. The key is never written to results, transcripts, or logs. (Housekeeping: the repo-root `.env` OpenRouter key was flagged for rotation in the 2026-07-02 readiness review — rotate before the first published run.)

### D4 — Agent loop and tool schema

Chat-completions loop with exactly two tools:

- `write_file(path, content)` — full-file write, repo-relative path, confined to the fixture sandbox (reject `..`/absolute paths)
- `done()` — model signals completion

**Fixture source reaches the model by embedding, not by tools.** The initial user message embeds the **complete fixture source** — every file under `src/` and `test/` (baseline and acceptance suites), each prefixed with its repo-relative path, plus `TASK.md` — rendered by a pinned template checked into `evals/prompts/`. No `read_file` or `list_files` tools are added: full embedding keeps the tool surface to the two above, keeps the context byte-identical across arms for a given task, and removes "did the model think to look?" as an arm-correlated variable. The acceptance tests are visible to the model this way (referenced in `TASK.md` and present in the embedded source), identical across all three arms — so every arm knows what "done" must satisfy.

The rendered prompt is **hashed (SHA-256) into the run manifest** so a published run proves the embedded source was the same across its arms. Because the prompt is byte-identical across arms, one hash per task is sufficient; the manifest records it.

Requests use temperature 0, `top_p` 1, and the `seed` parameter where the provider honors it. Each run executes in a throwaway copy of the fixture. Every run appends a full JSONL transcript (messages, tool calls, gate verdicts, per-call token usage from the API's `usage` field) under `evals/results/<run-id>/`.

### D5 — Arm C drives the real shipped binary

After each `write_file`, the harness runs, from the fixture root:

```
ironlint check --file <path> --content - --event write
```

with the proposed content on stdin — the exact shipped ABI, not a simulation. Exit-code handling per the locked contract:

- `0` → write lands, tool result "ok"
- `2` → write does **not** land; the verdict's Block messages become the tool result; counts one feedback round
- `1` or `3` → harness bug or broken check: **abort the run loudly** and mark it `harness-error`. A broken gate must never be scored as a pass (fail-loud, per the readiness-review theme).

Because D4 runs each rep in a throwaway fixture copy at a fresh path, and ironlint's trust store is keyed by the config's **canonical absolute path**, blessing the checked-in fixture does nothing for the copies. `ironlint trust` therefore runs **per run-copy during run setup, after the copy is created** (check fails closed on untrusted config). Consequences: this churns the trust store over thousands of runs (harmless — entries accumulate but are never read again after their run), and, if runs are ever parallelized, the bless step must be **serialized** to avoid contention on the trust file's atomic writes. The ironlint binary version is recorded in the run manifest.

### D6 — Model IDs are runtime inputs, not spec constants

The bench is model-agnostic by design: `--model <id>` (repeatable) selects models; the run manifest records exactly what was used. Published headline runs pin their model IDs in the results artifact, not in this spec — model catalogs drift, benchmarks shouldn't. Policy for the headline matrix: one small open-weights model + one frontier model, so the write-up can show even strong models slop without gates.

## Task corpus

Twelve hand-authored tasks in **plain Node.js (ESM)** — no compile step; tests run with `node:test`; prerequisites are Node, the pinned npm deps in `evals/tasks/`, a pinned Biome standalone binary (fetched by a setup script, version in the manifest), and the API key.

Each task is a small fixture repo (layered `src/`, two `node:test` suites — `test/baseline/` and `test/acceptance/` — and a `.ironlint.yml`) engineered to tempt one slop mode. Because D4 embeds the entire fixture source into the prompt, fixtures must stay small enough to embed whole: total fixture source (`src/` + `test/` + `TASK.md`) should be a few hundred lines, comfortably in-context. Four tasks per pillar:

| Pillar | Gate check | Tasks (tempting shape) |
|---|---|---|
| **Duplication / reuse** (GitClear's gap) | `jscpd` against a **shadow overlay** of `src/` with the proposed write applied (see "Authoring cross-file checks under pre-write" below), failing if duplicated-lines percentage exceeds the fixture's committed baseline | `endpoints-trio` (add three similar REST endpoints; a `validateBody` helper already exists), `report-formats` (CSV/JSON/markdown export; shared row-serializer exists), `retry-wrappers` (add retry to three client calls; `withRetry` exists), `validators` (validate three entity types; schema helpers exist) |
| **Complexity** (the validated metric) | Biome with `noExcessiveCognitiveComplexity` (max 15) + default lint rules | `config-parser` (layered config with overrides), `state-machine` (order-status transition rules), `pricing-rules` (tiered discounts with exceptions), `log-filter` (multi-criteria filtering with negation syntax) |
| **Architecture** | `dependency-cruiser` layer rules (e.g., routes must not import the db module directly) | `route-feature` (add a route where importing the db directly is the lazy path), `notify-channel` (new channel behind an existing port/adapter seam), `cache-layer` (add caching without domain→infra imports), `cli-command` (new command that must go through the service layer) |

Functional tests are **not** part of the gate — the gate is static-only, matching the blog's pillars. The two suites serve different measurement roles: the **baseline** suite is the regression guardrail, and the **acceptance** suite is the task-success co-primary (see Metrics). Both are run by the harness on final code.

Each fixture's `.ironlint.yml` is also the single source for arm B's rule text: a small renderer converts the checks (plus the underlying Biome/depcruise/jscpd rule configs) into the prose rule list injected into B's system prompt, so B and C are always enforcing/describing the identical rule set — no drift.

### Authoring cross-file checks under pre-write

Arm C gates **pre-write** (see Gate posture): when a check runs, the proposed content is **not on disk** — the old file is. Cross-file checks that scan `src/` naively will either miss the proposed content entirely or, worse, flag every unchanged line as duplication of itself. The checks must be authored to handle this. The common thread is `$IRONLINT_TMPFILE`: ironlint materializes the proposed content as a temp file sibling to `$IRONLINT_FILE`, same extension, and the check script builds its view of `src/` around it.

- **jscpd (duplication).** The check script builds a **shadow overlay**: copy `src/` to a fresh temp dir, write the proposed content (read from stdin / `$IRONLINT_TMPFILE`) at `$IRONLINT_FILE`'s path relative to `$IRONLINT_ROOT`, run jscpd against the overlay, compare the duplicated-lines percentage against the committed baseline, then clean up the overlay. The baseline is computed by the same script over the pristine fixture so numerator and denominator match. A naive "jscpd over `src/`" would either miss the new file or double-count the old one against the tmpfile — the overlay is the fix.
- **dependency-cruiser (architecture).** Layer rules must be written as **directory globs** (e.g., `src/routes/**`), never exact filenames, so they match `$IRONLINT_TMPFILE` (a sibling in the same directory with a generated name). Note that pre-write depcruise can only validate the proposed file's *own* imports — it cannot resolve imports in other files that reference the proposed file. That is sufficient for the "routes must not import db" rule family (the check runs on the route file, not the db file), and the spec says so.
- **Biome (complexity).** Per-file on the stdin / `$IRONLINT_TMPFILE` content. No cross-file issue — cognitive complexity is a single-file metric. Stated here so the asymmetry across pillars is documented, not implicit.

## Metrics

All measurement is deterministic — no LLM judges anywhere. The measurement layer practices what the thesis preaches.

The headline is a **joint outcome, not a single metric.** C wins only if it holds all three simultaneously: (1) fewer quality violations in landed code, (2) acceptance-suite pass (task success), (3) completion within T/R budget. Presenting these per-arm in one table makes "wins on violations but didn't finish the task" visible — a C run that blocks everything and lands nothing has zero violations but fails the joint outcome.

- **Co-primary — violation count in landed code.** Run the same pinned tools (Biome, jscpd, dependency-cruiser) with the same rule configs over the final fixture state and count violations (counts, not just exit codes, for granularity). Reported per arm as mean ± CI. Caveat: in arm C, landed code passed these exact tools by construction (the gate ran them pre-write), so a low violation count is guaranteed, not discovered — the near-tautology is acknowledged and defused by the joint outcome above and by the first-attempt violation rate below.
- **Co-primary — task success (acceptance-suite pass on final code).** The acceptance suite covers the new functionality the task asks for; passing it is what "the task was accomplished" means. This is a co-primary outcome, not a guardrail — a run that lands zero writes (e.g., `blocked-incomplete`) leaves the fixture untouched, the acceptance suite fails, and the run scores as a **task failure** regardless of its violation count. This closes the "doing nothing scores perfectly" hole.
- **Run classification.** `completed` / `blocked-incomplete` (arm C ran out of R with writes still blocked — slop never landed, but the task was not accomplished; reported separately, not hidden, and scored as a task failure via the acceptance suite) / `budget-exhausted` / `api-error` / `harness-error`.
- **Guardrail — regression (baseline-suite pass on final code).** The baseline suite passes on the unmodified fixture; a drop on final code means the arm broke existing functionality. This is the regression guardrail, distinct from the acceptance co-primary.
- **Mechanism metric — first-attempt violation rate.** Computable from transcripts at zero extra cost: for arm C, the fraction of `write_file` attempts the gate blocked, plus the violation counts in those blocked writes. This is the direct evidence for the arXiv mechanism (the model generates slop it cannot self-detect but fixes on signal) and belongs in the headline write-up alongside the landed-code count. The same metric is not available for arms A/B (no gate to veto writes), which is itself the point.
- **Secondary.** Tokens in/out (from API `usage`), feedback rounds used (review-round counts for B, blocked-write counts for C — see Budget parity), wall-clock, LOC added, per-function cognitive-complexity distribution.

**Statistics.** K = 5 reps per (task × arm × model) — temperature 0 is not deterministic across OpenRouter backends, so repetition + published transcripts stand in for bit-reproducibility. **Unit of analysis:** reps are averaged within task first; the sample is **n = 12 task-level paired differences** per arm-pair per model. All CIs and the Wilcoxon signed-rank test operate on these task-level values (preventing pseudo-replication over 60 reps). Aggregation: bootstrap 95% CIs (fixed RNG seed so the report itself is reproducible), Wilcoxon signed-rank as a secondary check. Per-task table plus aggregate in the report.

**Non-inferiority for task success.** "No loss in functional task success" is a non-inferiority claim, not an equivalence claim — a non-significant difference with n = 12 is weak evidence of equivalence. Pre-registered margin: C is declared "no loss" only if the 95% CI of the C−B acceptance-pass-rate difference excludes a drop worse than **10 percentage points** (fixed in the spec before any data exists; substitute a principled alternative margin only if justified before data collection).

**Cost envelope.** 12 tasks × 3 arms × 5 reps × ~8 calls ≈ 1,440 API calls per model. Small open model ≈ $5–15; frontier model ≈ $50–150. `--reps 1` is the development smoke mode.

## Run protocol

```
bench run --model <id> [--arm a|b|c] [--task <id>] [--reps N] [--base-url URL]
bench report <results-dir>    # aggregates JSONL → markdown tables + summary.json
```

`bench run` writes `evals/results/<run-id>/manifest.json` (git SHA, ironlint version, Biome version, npm lockfile hash, model IDs, base URL host, **rendered-prompt hash** — SHA-256 of the full embedded fixture source per D4, one per task, byte-identical across arms — T/R/K parameters) plus per-run transcript JSONL. `bench report` computes all statistics from artifacts alone — anyone can re-derive the tables from a published results directory without rerunning the API.

## Error handling

- **API 429/5xx** — exponential backoff, max 5 retries; then the run is marked `api-error`, excluded from aggregates, and counted in the report (silent exclusion would bias results). **api-error exclusions are reported per arm**, not just globally: arm C makes more API calls per run (gate-feedback turns) and therefore has more exposure to provider errors, so a global exclusion rate could selectively prune C. The report must show exclusion counts per arm so readers can verify the rate is arm-balanced.
- **Malformed tool call** — one reprompt containing the schema error; counts against the turn cap T.
- **ironlint exit 1/3** (config/load error or crashed check) — abort loudly as `harness-error`; never scored.
- **Fixture sandbox escape attempts** (absolute or `..` paths in `write_file`) — rejected with an error tool result; counts against T.

### Security — scoring executes model-written code

The scoring step runs `node --test` and the measurement tools (Biome, jscpd, dependency-cruiser) over the model's final code. That is arbitrary code execution of untrusted model output on the host, and `BENCH_API_KEY` is in the harness's environment. The scoring subprocess therefore runs with a **scrubbed environment** — no `BENCH_API_KEY`, minimal `PATH` (only what the tools need), and ideally no network access. For published runs, run the scoring step inside a container. Cheap to specify; embarrassing to omit from a published artifact.

## Testing

- Unit tests (Rust, in-crate): budget accounting (T/R enforcement per arm), gate exit-code classification, transcript/manifest serialization, report math (bootstrap with fixed seed → exact expected output), rule-text renderer (`.ironlint.yml` → arm B prose).
- **Replay mode**: `--mock-model <fixture>` feeds canned API responses through the full loop — end-to-end harness tests with zero API calls; doubles as the CI smoke test and the task-authoring dev loop.
- Each task fixture's **baseline** `node:test` suite must pass on the *unmodified* fixture (a task whose baseline is broken measures nothing).
- Each task fixture's **acceptance** suite must **fail** on the *unmodified* fixture (a task whose acceptance passes before the model touches it measures nothing — the task is already done). This is the authoring invariant that makes task success a meaningful co-primary: if acceptance passes pre-edit, the bench cannot distinguish "the model did the task" from "the task was trivial."

## Non-goals

- Not a general agent benchmark; it measures one variable (feedback source) on slop-tempting tasks.
- No LLM-judge scoring, by design.
- No multi-language matrix initially (Node-only; a second stack is future work).
- No Claude Code headless arm in v1 — a real-harness appendix run is a possible follow-up, as is a 4th arm **B′ — inline self-review**: after each `write_file`, the model is prompted to check that file against the rules before it lands (the write is held until the self-review passes). B′ matches C's timing and enforcement point exactly and differs only in verdict source (self vs. deterministic), so it is the designed answer to the decomposition critique — isolating the verdict-source strand of the bundle. It is future work for v1 but named here so reviewers see the critique is anticipated, not ignored.
- The bench does not gate on or measure ironlint's own performance overhead (covered by `specs/2026-07-01-performance-dx-audit.md`).

## Resolved decisions

- Three arms with steelmanned B (rules-in-prompt + self-review) — user-selected.
- Purpose-built minimal loop over Claude Code headless — default taken per recommendation.
- In-repo `evals/` over a separate repo — results live next to the tool they prove.
- Model IDs live in run manifests, not the spec (D6).
