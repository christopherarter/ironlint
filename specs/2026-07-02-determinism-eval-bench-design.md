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
| **C — ironlint gated** | task only (no rule text) | every `write_file` is piped through the real `ironlint check` binary; exit 2 returns the check's Block message as the tool result and the write does **not** land; up to **R** blocked-write feedbacks per run | the deterministic verdict as the only added variable |

If C beats B, the only explanatory variable is the *source* of the feedback: deterministic verdict vs. self-assessment. That is the arXiv mechanism, isolated.

**Budget parity.** One global turn cap **T = 20** tool calls per run and one feedback-round cap **R = 5** per run, identical across arms (A simply never uses feedback rounds). B's feedback arrives post-completion and C's arrives inline per-write — that asymmetry is inherent to what each design *is*, so parity is defined as: same T, same R, and actual token usage reported per arm so readers can verify neither arm got a hidden budget advantage.

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
      src/  test/       # fixture code + node:test suite
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

Setup blesses each fixture's config via `ironlint trust` (check fails closed on untrusted config). The ironlint binary version is recorded in the run manifest.

### D6 — Model IDs are runtime inputs, not spec constants

The bench is model-agnostic by design: `--model <id>` (repeatable) selects models; the run manifest records exactly what was used. Published headline runs pin their model IDs in the results artifact, not in this spec — model catalogs drift, benchmarks shouldn't. Policy for the headline matrix: one small open-weights model + one frontier model, so the write-up can show even strong models slop without gates.

## Task corpus

Twelve hand-authored tasks in **plain Node.js (ESM)** — no compile step; tests run with `node:test`; prerequisites are Node, the pinned npm deps in `evals/tasks/`, a pinned Biome standalone binary (fetched by a setup script, version in the manifest), and the API key.

Each task is a small fixture repo (layered `src/`, working `node:test` suite, `.ironlint.yml`) engineered to tempt one slop mode. Four tasks per pillar:

| Pillar | Gate check | Tasks (tempting shape) |
|---|---|---|
| **Duplication / reuse** (GitClear's gap) | `jscpd` over `src/`, failing if the duplicated-lines percentage exceeds the fixture's committed baseline | `endpoints-trio` (add three similar REST endpoints; a `validateBody` helper already exists), `report-formats` (CSV/JSON/markdown export; shared row-serializer exists), `retry-wrappers` (add retry to three client calls; `withRetry` exists), `validators` (validate three entity types; schema helpers exist) |
| **Complexity** (the validated metric) | Biome with `noExcessiveCognitiveComplexity` (max 15) + default lint rules | `config-parser` (layered config with overrides), `state-machine` (order-status transition rules), `pricing-rules` (tiered discounts with exceptions), `log-filter` (multi-criteria filtering with negation syntax) |
| **Architecture** | `dependency-cruiser` layer rules (e.g., routes must not import the db module directly) | `route-feature` (add a route where importing the db directly is the lazy path), `notify-channel` (new channel behind an existing port/adapter seam), `cache-layer` (add caching without domain→infra imports), `cli-command` (new command that must go through the service layer) |

Functional tests are **not** part of the gate — the gate is static-only, matching the blog's pillars. Tests are the guardrail metric, run by the harness on final code.

Each fixture's `.ironlint.yml` is also the single source for arm B's rule text: a small renderer converts the checks (plus the underlying Biome/depcruise/jscpd rule configs) into the prose rule list injected into B's system prompt, so B and C are always enforcing/describing the identical rule set — no drift.

## Metrics

All measurement is deterministic — no LLM judges anywhere. The measurement layer practices what the thesis preaches.

- **Primary — violation count in landed code.** Run the same pinned tools (Biome, jscpd, dependency-cruiser) with the same rule configs over the final fixture state and count violations (counts, not just exit codes, for granularity). Reported per arm as mean ± CI.
- **Run classification.** `completed` / `blocked-incomplete` (arm C ran out of R with writes still blocked — slop never landed; reported separately, not hidden) / `budget-exhausted` / `api-error` / `harness-error`.
- **Guardrail — functional success.** `node --test` pass rate on final code. Proves quality didn't come at correctness's expense.
- **Secondary.** Tokens in/out (from API `usage`), feedback rounds used, wall-clock, LOC added, per-function cognitive-complexity distribution.

**Statistics.** K = 5 reps per (task × arm × model) — temperature 0 is not deterministic across OpenRouter backends, so repetition + published transcripts stand in for bit-reproducibility. Aggregation: paired per-task arm differences, bootstrap 95% CIs (fixed RNG seed so the report itself is reproducible), Wilcoxon signed-rank as a secondary check. Per-task table plus aggregate in the report.

**Cost envelope.** 12 tasks × 3 arms × 5 reps × ~8 calls ≈ 1,440 API calls per model. Small open model ≈ $5–15; frontier model ≈ $50–150. `--reps 1` is the development smoke mode.

## Run protocol

```
bench run --model <id> [--arm a|b|c] [--task <id>] [--reps N] [--base-url URL]
bench report <results-dir>    # aggregates JSONL → markdown tables + summary.json
```

`bench run` writes `evals/results/<run-id>/manifest.json` (git SHA, ironlint version, Biome version, npm lockfile hash, model IDs, base URL host, prompt hashes, T/R/K parameters) plus per-run transcript JSONL. `bench report` computes all statistics from artifacts alone — anyone can re-derive the tables from a published results directory without rerunning the API.

## Error handling

- **API 429/5xx** — exponential backoff, max 5 retries; then the run is marked `api-error`, excluded from aggregates, and counted in the report (silent exclusion would bias results).
- **Malformed tool call** — one reprompt containing the schema error; counts against the turn cap T.
- **ironlint exit 1/3** (config/load error or crashed check) — abort loudly as `harness-error`; never scored.
- **Fixture sandbox escape attempts** (absolute or `..` paths in `write_file`) — rejected with an error tool result; counts against T.

## Testing

- Unit tests (Rust, in-crate): budget accounting (T/R enforcement per arm), gate exit-code classification, transcript/manifest serialization, report math (bootstrap with fixed seed → exact expected output), rule-text renderer (`.ironlint.yml` → arm B prose).
- **Replay mode**: `--mock-model <fixture>` feeds canned API responses through the full loop — end-to-end harness tests with zero API calls; doubles as the CI smoke test and the task-authoring dev loop.
- Each task fixture's own `node:test` suite must pass on the *unmodified* fixture (a task whose baseline is broken measures nothing).

## Non-goals

- Not a general agent benchmark; it measures one variable (feedback source) on slop-tempting tasks.
- No LLM-judge scoring, by design.
- No multi-language matrix initially (Node-only; a second stack is future work).
- No Claude Code headless arm in v1 — a real-harness appendix run is a possible follow-up, as is a 4th arm (generic self-review without rule text) if reviewers ask for it.
- The bench does not gate on or measure ironlint's own performance overhead (covered by `specs/2026-07-01-performance-dx-audit.md`).

## Resolved decisions

- Three arms with steelmanned B (rules-in-prompt + self-review) — user-selected.
- Purpose-built minimal loop over Claude Code headless — default taken per recommendation.
- In-repo `evals/` over a separate repo — results live next to the tool they prove.
- Model IDs live in run manifests, not the spec (D6).
