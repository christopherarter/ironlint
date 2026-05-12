# Plans

Implementation plans Hector is built from. Each plan is a self-contained, multi-step, checkbox-tracked design + execution doc following the `superpowers:writing-plans` format. Plans are executed via `superpowers:executing-plans` (inline) or `superpowers:subagent-driven-development` (subagent-per-task).

A plan owns its own progress via its checkboxes — that's the source of truth. This README is a navigation surface; keep it in sync by hand when state changes meaningfully (new plan added, plan completed, priority shifted). The `Future` section below is the closest thing to a backlog: short bullets for work that hasn't graduated to a plan file yet.

**Layout:**

- `plans/*.md` — in-flight or queued
- `plans/archive/*.md` — completed (frozen design records, useful as "how was X built" context)

## Active

The 0.2.0 cohort closes the bully-parity gaps. Sequencing matters: A1 lands the security boundary that A2 and A3 lean on. See [`specs/2026-05-12-bully-parity-closures.md`](../specs/2026-05-12-bully-parity-closures.md) for the spec.

| Priority | Plan | Goal | Steps | Order |
|---|---|---|---|---|
| 🔴 security | [A1 — Prompt-Injection Defense](2026-05-12-hector-a1-prompt-injection.md) | Wrap rule policy in `<TRUSTED_POLICY>` and user content in `<UNTRUSTED_EVIDENCE>` so adversarial diffs can't smuggle rule-list content into the prompt. | 0 / 16 | first |
| 🔴 cost | [A2 — Skip Patterns](2026-05-12-hector-a2-skip-patterns.md) | Default skip for lockfiles, generated code, build artifacts. Project `skip:` in `.hector.yml` + `~/.hector-ignore` for user-global skips. | 0 / 24 | after A1 |
| 🔴 cost | [A3 — Diff Pre-Filter](2026-05-12-hector-a3-diff-prefilter.md) | Locally short-circuit `engine: semantic` when the diff can't fire the rule (empty / whitespace / comment-only / pure-deletion against an "avoid X" rule). | 0 / 40 | after A2 |

## Future

Ideas that haven't graduated to plans. When something here has enough definition to write a plan against, lift it into a dated plan file.

- _(nothing queued yet — add bullets here as the next cohort takes shape)_

## Archive

Completed plans live in [`archive/`](archive/). They're frozen design records.

- [`2026-05-11-hector-0.1a-foundation`](archive/2026-05-11-hector-0.1a-foundation.md) — workspace skeleton, script engine, trust gate.
- [`2026-05-11-hector-0.1b-engines`](archive/2026-05-11-hector-0.1b-engines.md) — `ast`, `semantic`, `session` engines + `init` / `migrate` / `baseline` / `session record` commands.
- [`2026-05-11-hector-0.1c-claude-code-adapter`](archive/2026-05-11-hector-0.1c-claude-code-adapter.md) — Claude Code adapter (`plugin.json`, PostToolUse + Stop hooks, ported skills).
- [`2026-05-12-hector-opencode-adapter`](archive/2026-05-12-hector-opencode-adapter.md) — OpenCode adapter at parity with the Claude Code adapter.
- [`2026-05-12-bug-audit-remediation`](archive/2026-05-12-bug-audit-remediation.md) — remediation campaign for the [P0/P1/P2 findings](../docs/audits/2026-05-12-bug-audit.md) from the 2026-05-12 audit.

## Conventions

- One feature/refactor per plan file. Filename: `YYYY-MM-DD-<short-slug>.md`.
- Plan header carries Goal, Architecture, Tech Stack, and (when relevant) Severity + Sequencing. See `superpowers:writing-plans` for the canonical format.
- Tasks are bite-sized (2–5 min) and checkbox-tracked. The TDD cycle (write failing test → verify red → minimal impl → verify green → commit) is the per-task default per repo rules.
- Substantive work — multi-file, multi-phase, architectural — earns a plan. A 3-line "fix X" doesn't; just commit it.
- When a plan ships, `git mv` it to `archive/` and strike its row from the table above.
