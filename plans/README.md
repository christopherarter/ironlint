# Plans

Implementation plans Hector is built from. Each plan is a self-contained, multi-step, checkbox-tracked design + execution doc following the `superpowers:writing-plans` format. Plans are executed via `superpowers:executing-plans` (inline) or `superpowers:subagent-driven-development` (subagent-per-task).

A plan owns its own progress via its checkboxes — that's the source of truth. This README is a navigation surface; keep it in sync by hand when state changes meaningfully (new plan added, plan completed, priority shifted). The `Future` section below is the closest thing to a backlog: short bullets for work that hasn't graduated to a plan file yet.

**Layout:**

- `plans/*.md` — in-flight or queued
- `plans/archive/*.md` — completed (frozen design records, useful as "how was X built" context)

## Active

_(nothing queued — H1/H2 plans land next, covering [`specs/2026-05-14-subagent-semantic-eval.md`](../specs/2026-05-14-subagent-semantic-eval.md))_

## Future

Ideas that haven't graduated to plans. When something here has enough definition to write a plan against, lift it into a dated plan file.

- **H1–H4 subagent semantic eval** ([spec](../specs/2026-05-14-subagent-semantic-eval.md)). Restores bully's Claude Code in-session subagent path so subscription users can run `engine: semantic` without an `ANTHROPIC_API_KEY`. H1 `--emit-semantic-payload` + H2 `record-verdict` are core scaffolding; H3 is the adapter mode; H4 is the docs walkback. H1 and H2 are independent and can ship in parallel.
- **D2 `hector coverage`** and **D3 `hector debt`** ([spec §D](../specs/2026-05-12-bully-parity-closures.md)) — telemetry-derived rule-coverage and tech-debt reports. D1 (typed telemetry) shipped; these consume it.
- **A4 `context.lines`** — per-rule context-line count override on the semantic prompt.
- **C5 `validate --execute-dry-run`** — invoke `script:` rules in a sandbox during `validate`, surface failures early.
- **F1 declarative session rules** — `when.changed_any` / `require.changed_any` as a deterministic alternative to LLM-driven session eval.
- **G1 trust+rules split CI lint** (stop-gap; full trust-model decision blocks 0.3 freeze).

### Shipped without a plan file

Small/medium changes that landed direct-to-`main` without a dedicated plan; recorded here so they aren't invisible.

- **2026-05-22** — E2 script-engine output modes (`Parsed` / `Passthrough`); see [`CHANGELOG.md`](../CHANGELOG.md#unreleased) and commit `3241026`.
- **2026-05-22** — OpenCode adapter pre-flight gate (moved to `tool.execute.before` + shadow-write + late-init fix); commit `069cc74`.
- **2026-05-22** — macOS capability-warning dedup (once per process instead of per script invocation); commit `f47ef82`.

## Archive

Completed plans live in [`archive/`](archive/). They're frozen design records.

- [`2026-05-11-hector-0.1a-foundation`](archive/2026-05-11-hector-0.1a-foundation.md) — workspace skeleton, script engine, trust gate.
- [`2026-05-11-hector-0.1b-engines`](archive/2026-05-11-hector-0.1b-engines.md) — `ast`, `semantic`, `session` engines + `init` / `migrate` / `baseline` / `session record` commands.
- [`2026-05-11-hector-0.1c-claude-code-adapter`](archive/2026-05-11-hector-0.1c-claude-code-adapter.md) — Claude Code adapter (`plugin.json`, PostToolUse + Stop hooks, ported skills).
- [`2026-05-12-hector-opencode-adapter`](archive/2026-05-12-hector-opencode-adapter.md) — OpenCode adapter at parity with the Claude Code adapter.
- [`2026-05-12-bug-audit-remediation`](archive/2026-05-12-bug-audit-remediation.md) — remediation campaign for the [P0/P1/P2 findings](../docs/audits/2026-05-12-bug-audit.md) from the 2026-05-12 audit.
- [`2026-05-12-hector-a1-prompt-injection`](archive/2026-05-12-hector-a1-prompt-injection.md) — `<TRUSTED_POLICY>` / `<UNTRUSTED_EVIDENCE>` sentinel boundary in semantic prompt.
- [`2026-05-12-hector-a2-skip-patterns`](archive/2026-05-12-hector-a2-skip-patterns.md) — built-in skip patterns + project `skip:` + `~/.hector-ignore`.
- [`2026-05-12-hector-a3-diff-prefilter`](archive/2026-05-12-hector-a3-diff-prefilter.md) — local `can_match_diff` short-circuit for `engine: semantic`; new `reason` field on telemetry; runner-side wiring.
- [`2026-05-12-hector-e1-baseline-checksum`](archive/2026-05-12-hector-e1-baseline-checksum.md) — `line_sha256` fingerprinting in `Baseline`; v1-format read tolerance; new `hector baseline refresh` subcommand.
- [`2026-05-12-hector-b1-parallel-rules`](archive/2026-05-12-hector-b1-parallel-rules.md) — rayon-driven parallel rule dispatch in `HectorEngine::check`; `execution.max_workers` config + `HECTOR_MAX_WORKERS` env override; deterministic output order via BTreeMap iteration.
- [`2026-05-12-hector-c4-check-flags`](archive/2026-05-12-hector-c4-check-flags.md) — `hector check --rule <id>` (repeatable) restricts evaluation upstream of the parallel pool; `--explain` prints a per-rule outcome report to stderr; `--print-prompt` renders the semantic prompt without dispatching to the LLM.
- [`2026-05-13-hector-d1-typed-telemetry`](archive/2026-05-13-hector-d1-typed-telemetry.md) — typed `LogEntry` enum (`session_init` / `check` / `semantic_verdict` / `semantic_skipped`) with `PerRuleRecord` nesting and a `read_all` legacy reader. Foundation for D2/D3.
- [`2026-05-13-hector-c1-doctor`](archive/2026-05-13-hector-c1-doctor.md) — `hector doctor` diagnostic subcommand: 9 checks (binary, config, parses, trust, schema, scope_globs, engines, adapter, runtime_state); JSON contract under `docs/doctor.md`; exit code 0 on pass-or-warn, 1 on any fail.
- [`2026-05-13-hector-c2-explain-guide`](archive/2026-05-13-hector-c2-explain-guide.md) — `hector explain <file>` and `hector guide <file>` read-only inspection subcommands; shared `scope_outcomes` helper in `hector-core`; JSON snapshots locked with insta.
- [`2026-05-13-hector-c3-show-resolved-config`](archive/2026-05-13-hector-c3-show-resolved-config.md) — `hector show-resolved-config` (TSV / YAML / JSON) with per-rule origin tracking; `extends::resolve_with_origin` core helper.

## Conventions

- One feature/refactor per plan file. Filename: `YYYY-MM-DD-<short-slug>.md`.
- Plan header carries Goal, Architecture, Tech Stack, and (when relevant) Severity + Sequencing. See `superpowers:writing-plans` for the canonical format.
- Tasks are bite-sized (2–5 min) and checkbox-tracked. The TDD cycle (write failing test → verify red → minimal impl → verify green → commit) is the per-task default per repo rules.
- Substantive work — multi-file, multi-phase, architectural — earns a plan. A 3-line "fix X" doesn't; just commit it.
- When a plan ships, `git mv` it to `archive/` and strike its row from the table above.
