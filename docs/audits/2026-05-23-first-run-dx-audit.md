# Hector first-run DX audit

**Date:** 2026-05-23
**Auditor:** Chris Arter
**Source:** First hands-on test of the shipped 0.1 binary + Claude Code adapter (H3) against a real project — `samplevu-next` (Next.js / TypeScript / pnpm workspaces / biome). Two annotated transcripts captured in `~/Documents/projects/samplevu-next/2026-05-23-163729-write-this-hectoryml-file.txt` and `~/Documents/projects/samplevu-next/2026-05-23-163844-make-an-update-to-try-and-blatantly-violate-the.txt`.

## What was tested

- `hector init` against a `pnpm` monorepo
- Adding a semantic rule + `llm.provider: claude-code-subagent`
- `hector trust` round-trip
- Hook firing on `Write`/`Edit` of source files
- Deliberate rule-violation edit to verify blocking

## Architectural decision recorded out of this audit

**Script-engine output defaults to `passthrough`, not `parsed`.** The user observed `parsed` mode mis-handling biome's pretty diagnostic frame — each line of biome's column header / source preview became a separate "violation". We will not chase a parser per tool (biome / eslint / ruff / clippy variants / mypy / golangci-lint / …). Bully's design is passthrough; we match it. `parsed` stays as opt-in for the formats we explicitly support. See [R4](#r4) for the implementation slice. Memory: `feedback_script_output_passthrough.md`.

## Findings

| ID | Severity | Title | Effort | Order |
|----|----------|-------|--------|-------|
| [R1](#r1) | High | `hector init` is monorepo-blind and doesn't detect existing linters | M | 5 |
| [R2](#r2) | Low | `model:` required-but-ignored under `claude-code-subagent` | S | 3a |
| [R3](#r3) | Med | Hook self-checks `.hector.yml` mid-edit → internal-error noise | XS | 2 |
| [R4](#r4) | High | Default script-rule output flips to `passthrough` | S | 1 |
| [R5](#r5) | Med | No config knob for evaluator-subagent model | S | 3b |
| [R6](#r6) | Med | Deferred semantic rules are invisible when a deterministic block fires | S | 4 |
| [R7](#r7) | Low | Hook output is doubled (raw JSON + formatted line) per block | S | 6 |
| [R8](#r8) | Low | Discoverability: subagent path requires reading source-repo docs | S | 7 (docs) |

Severity = blast radius on a real user, not engineering complexity. Effort: XS = ≤30 min, S = ≤2 h, M = half day, L = full day+.

---

### R1 — `hector init` is monorepo-blind and doesn't detect existing linters {#r1}

**Evidence (transcript 1, lines 42–95):** `hector init` scaffolded `scope: ["src/**/*.ts", ...]`. The repo's source lives under `apps/**/src/` and `packages/**/src/`, so every scaffolded glob matched zero files. The init also wrote a `no-console-log` grep rule even though biome is configured and biome's `noConsole` lint catches the same thing — when the user later added a `biome-check` rule, the two rules fired in lockstep on the same violation.

**Why it matters:** Init is the user's first impression. A scaffold that matches zero files reads as "broken out of the box." Duplicating linter rules makes the policy feel naive about the stack.

**Fix:**
- Detect workspace shape before generating scopes: `pnpm-workspace.yaml`, `package.json` `workspaces` field, `Cargo.toml` `[workspace]`, `go.work`. If present, generate scopes from each workspace member's source root.
- Detect existing config files in repo root: `biome.json` / `biome.jsonc`, `.eslintrc*`, `eslint.config.*`, `ruff.toml` / `pyproject.toml` (`[tool.ruff]`), `clippy.toml`. When present, either (a) skip script rules that duplicate a known check, or (b) scaffold a *passthrough* wrapper rule that shells out to the tool the project already uses.
- Scaffold a commented-out `llm:` block + one example commented-out semantic rule so the agentic path is discoverable without docs.

**Files likely touched:** `crates/hector-cli/src/commands/init.rs` (or wherever init lives), associated fixtures + integration test.

**Test approach:** Add fixtures for (pnpm workspace, no linter), (pnpm + biome), (Cargo workspace + clippy), (single-package npm + eslint). Assert the generated `.hector.yml` has correct scopes and doesn't duplicate detected linters.

---

### R2 — `model:` required-but-ignored under `claude-code-subagent` {#r2}

**Evidence (transcript 1, lines 142–168):** User wrote `model: ignored` literally because the field is required by the parser but unused for the subagent provider.

**Why it matters:** Required-but-meaningless fields read as "the author didn't finish this." Confusing for a marquee config surface.

**Fix:** Make `model` optional in the config parser when `provider == claude-code-subagent`. If present, ignore it with a soft warning to stderr on load (`"model: <x> ignored — subagent provider uses Claude Code's session model"`). Default `Some(_)` to `None` when `claude-code-subagent` is the provider.

**Files likely touched:** `crates/hector-core/src/config/types.rs` (LlmConfig parsing), `crates/hector-core/src/llm/mod.rs` (provider construction), validate-time check in `runner.rs`.

**Test approach:** Round-trip a config with `provider: claude-code-subagent` and no `model:` field; assert load succeeds. Round-trip with `model: foo` and assert the warning fires.

---

### R3 — Hook self-checks `.hector.yml` mid-edit → internal-error noise {#r3}

**Evidence (transcript 1, lines 96–97 and 168):** Editing `.hector.yml` itself triggered `PostToolUse:Edit` → hook → `hector check --file .hector.yml` → exit 1 (file is mid-edit, trust gate fails). Twice in the transcript. The user saw scary "internal error" messages while doing something completely normal.

**Why it matters:** This is a footgun specifically on the path of "user is iterating on their policy." The hook should never error on the file that *is* the policy.

**Fix:** In `adapters/claude-code/hooks/hook.sh` and `adapters/opencode/src/index.ts`, short-circuit when the changed file is the policy file itself (`.hector.yml` or `.bully.yml`). Exit 0 with no output. Optional: log a one-liner stderr note (`hector: skipping self-check of policy file`) gated behind a debug env var.

**Files likely touched:** `adapters/claude-code/hooks/hook.sh`, `adapters/opencode/src/index.ts`, regression test under each adapter's tests.

**Test approach:** Add a scenario to `adapters/claude-code/tests/subagent_mode.sh` (and the opencode equivalent) that pipes a `PostToolUse` event for `.hector.yml`. Assert exit 0 and empty stdout/stderr.

---

### R4 — Default script-rule output flips to `passthrough` {#r4}

**Evidence (transcript 2, lines 38–90):** A single biome run on a 4-line edit produced 15+ "violations" — each was one line of biome's pretty-printed diagnostic frame (`"2 lint/suspicious/noConsole  FIXABLE  ━━━━━━━━━━━━━━━"`, source preview lines, etc.) wrongly parsed as `Violation.message`.

**Why it matters:** Architectural — we will not be the team that ships a parser for every linter on earth. Bully's passthrough model is correct; the consuming agent (or downstream tooling) can interpret raw diagnostic text. Today's default mismatch makes hector's output look broken on the first real linter.

**Fix:**
- Flip `Rule.output` default from `Parsed` → `Passthrough` in `crates/hector-core/src/config/types.rs`.
- Update all in-tree fixtures that relied on the old default to opt into `output: parsed` explicitly.
- Update `engine/script.rs` and `engine/output.rs` to keep the `parsed` arm intact (don't delete; some users may want it for the formats we already know).
- Update `hector init` scaffold to never emit `output: parsed`.
- CHANGELOG entry under Unreleased: "Breaking (config): `output:` default changed from `parsed` → `passthrough`."
- Document in `docs/engines.md` (or wherever the script-engine reference lives) that `parsed` is opt-in and lists the supported formats; new formats are explicitly out of scope for now.

**Files likely touched:** `crates/hector-core/src/config/types.rs`, `crates/hector-core/src/engine/script.rs`, `crates/hector-core/src/engine/output.rs`, `crates/hector-cli/src/commands/init.rs` scaffold, fixtures in `tests/fixtures/`, snapshot tests, `CHANGELOG.md`, `docs/`.

**Test approach:** Update existing parsed-mode tests to opt in explicitly via `output: parsed`. Add a regression test that loads a config with no `output:` field, runs against a fixture, and asserts the violation message is the verbatim stdout/stderr.

---

### R5 — No config knob for evaluator-subagent model {#r5}

**Evidence (transcript 1, lines 207–270):** User asked "can the evaluator use Haiku?" Answer required editing `adapters/claude-code/agents/hector-evaluator.md` frontmatter directly — and they had to be warned that the dev-repo copy isn't necessarily the same file Claude Code loads at runtime.

**Why it matters:** Evaluator model choice is a cost/speed/quality lever a user should be able to pull from the policy file, not by editing a plugin's internal markdown. The "which copy of the file" gotcha makes this brittle.

**Fix:**
- Add optional `llm.evaluator_model: haiku | sonnet | opus` to the config schema.
- When `provider == claude-code-subagent` and `evaluator_model` is set, propagate it through the deferred-payload envelope so the hector skill can dispatch the subagent with a model override.
- Skill update (`adapters/claude-code/skills/hector/SKILL.md`) to read the override from the envelope (if exposed) and pass `--model <x>` (or equivalent) on the dispatch. May require adding the field to `DeferredVerdict` envelope; if so, bump `DEFERRED_SCHEMA_VERSION`.
- Doc the field in the adapter README.

**Files likely touched:** `crates/hector-core/src/config/types.rs`, `crates/hector-core/src/verdict_deferred.rs`, `adapters/claude-code/skills/hector/SKILL.md`, `adapters/claude-code/README.md`, `docs/emit-semantic-payload.md`.

**Test approach:** Round-trip a config with `evaluator_model: haiku`; assert envelope carries it. Update the subagent-mode adapter test to assert the dispatch line includes the override when the field is set.

---

### R6 — Deferred semantic rules are invisible when a deterministic block fires {#r6}

**Evidence (transcript 2, lines 92–101):** User edited a file that violated both `no-console-log` (script, error) and `no-todo-comment` (semantic, warning). The hook blocked on the script violations and never surfaced the semantic rule — from the user's POV, the TODO comment slipped through despite being defined in policy. Author's note in the transcript: "The semantic no-todo-comment rule (warning) didn't surface in this hook output."

**Why it matters:** Silent omission of a configured rule is the worst possible failure mode for a policy tool. If the user defined it and the file matched the scope, *some* signal should reach them.

**Fix:** When `--emit-semantic-payload` is on and a deterministic block also fires:
- Continue to exit 2 (block).
- Include the deferred rules in the verdict JSON under a new top-level `deferred_rules: [...]` array (or similar — pick the cleanest place that doesn't break the locked verdict shape; possibly extend the deterministic block's verdict, possibly emit a sibling envelope).
- The Claude Code skill should mention the deferred rules in its block-message so the user knows they were skipped, not silently passed.

**Files likely touched:** `crates/hector-core/src/runner.rs`, `crates/hector-core/src/verdict.rs` (with a `SCHEMA_VERSION` bump if the deterministic verdict shape changes), `crates/hector-core/src/verdict_deferred.rs`, `adapters/claude-code/hooks/hook.sh`, `adapters/claude-code/skills/hector/SKILL.md`.

**Test approach:** Add a runner test: config with one script (block) + one semantic rule, both match the file, both fire. Assert exit 2 + verdict lists both as having matched, with the semantic one tagged as deferred (not evaluated).

---

### R7 — Hook output is doubled (raw JSON + formatted line) per block {#r7}

**Evidence (transcript 2, lines 38–90):** Two `PostToolUse:Edit hook returned blocking error` headers fire per block — one carrying the full verdict JSON on a single line, another carrying the formatted `AGENTIC LINT -- blocked` summary. macOS capability warning also still appears occasionally in user-facing output despite the per-process dedup landed last week.

**Why it matters:** Visual noise is a tax on every blocked edit. Two structurally different messages for one event makes the output feel buggy. The capability warning leak suggests dedup isn't surviving subprocess invocation.

**Fix:** This one needs investigation before a fix lands.
- Read both adapters' hook output paths end to end. Determine whether the doubling is (a) hook emitting both, (b) Claude Code rendering both separately, or (c) something else.
- Pick one canonical block-output shape (formatted text by default, JSON behind `--format json`).
- Add a regression test: pipe a synthesized PostToolUse event into the hook, assert exactly one block message in the response.
- Trace the capability-warning leak: confirm whether it fires per `hector` invocation or per `bash` invocation, and whether the hook's multiple `hector` calls (record + check) trigger it twice.

**Files likely touched:** `adapters/claude-code/hooks/hook.sh`, `crates/hector-core/src/engine/capability.rs` warning dedup, regression tests.

**Test approach:** Investigation first. Then test depends on what's found.

---

### R8 — Discoverability: subagent path requires reading source-repo docs {#r8}

**Evidence (transcript 1, line 132–140):** User had to read `docs/emit-semantic-payload.md` from the source repo path to discover the subagent-mode shape.

**Why it matters:** Low-severity but accumulates. Real users won't have the source repo checked out.

**Fix (later):**
- `hector explain --llm` (or `hector doctor`) prints the resolved `llm:` block + a one-liner per provider on what it does.
- README + adapter READMEs include the subagent config snippet inline.
- Eventually: hector.dev / docs site, referenced from `--help` / `doctor`. Out of scope for 0.2.

**Defer:** Bundle with whatever docs polish lands before 0.2 release.

---

## Suggested execution order

The architectural pivot (R4) comes first because R1 will lean on it (init scaffold for biome should use passthrough). The quick win (R3) comes second. The config-surface cleanup (R2 + R5) batch together since they touch the same parser arm. R6 and R1 are the medium ones. R7 needs investigation before code. R8 is docs and defers.

1. **R4** — flip `output:` default to passthrough (architectural)
2. **R3** — hook skips `.hector.yml` (quick win, removes scary errors)
3. **R2 + R5** — bundled: model optional under subagent + new `evaluator_model` knob
4. **R6** — surface deferred rules on blocked verdicts
5. **R1** — init detects monorepo + existing linters
6. **R7** — investigate hook output doubling + capability warning leak
7. **R8** — docs polish (defer to pre-0.2 release work)

Each remediation is small enough to ship as its own commit/PR. Group execution via subagent-driven-development: one subagent per remediation, two-stage review per the skill.

## Out of scope

Things noticed during the audit but explicitly *not* in this remediation set:
- Reorganizing the script-engine output module to support pluggable parsers (we're deliberately not growing that surface — see R4).
- Adding new providers (the `claude-code-subagent` provider works; this audit is about polishing the path, not extending it).
- Plugin distribution mechanics (the "edit dev repo vs installed copy" gotcha in R5 surfaces a real problem but the fix is the config knob, not the distribution mechanism).
