# `hector init` onboarding clarity — design

**Date:** 2026-07-01
**Status:** approved, pre-implementation
**Touches:** `hector-core::adapter` (ops/registry), `hector-cli::commands::init::onboard`

## Problem

`hector init`'s harness-onboarding phase does not make it obvious **what** gets
installed, and it treats the two entry paths inconsistently:

- **Explicit `--harness <name>`** installs *silently* — no preview, no
  confirmation. The user learns what landed only from the terse per-harness
  result lines printed *after* the fact.
- **Auto-detect** prompts, but the prompt (`Install hector hooks into
  claude-code, pi? [Y/n]`) says "hooks" — understating it, since each harness
  also gets the `hector-config` authoring **skill** — and names no paths, so the
  user can't see what files are about to be written or which settings file is
  patched.
- Nothing signals *why* a harness is in the set: detected on disk vs. named on
  the command line.

The machinery to show exactly what lands already exists — the `--dry-run` branch
computes every `write <path>` / `patch <path> [key]` — but it is only reachable
via an explicit flag and is emitted as pre-formatted strings, never shown before
a real install.

## Goals

1. Before any install, render a **per-file plan** grouped per harness, so the
   user sees every file written / settings key patched.
2. Tag each harness with its **source**: `detected` vs `requested`.
3. **Unify** the two paths: explicit and auto-detect both render the plan, then
   confirm (`[Y/n]`, default yes; `--yes` skips).
4. Make it **prettier**: a section header, a tree layout, aligned step-kind
   columns, and light TTY-gated color.

Non-goals: changing *what* gets installed, the trust model, the exit codes, or
the post-install result lines. This is presentation + flow only.

## Design

### 1. Structured plan model (core)

Replace the stringly-typed dry-run output with a structured plan so the CLI owns
all formatting. In `hector-core::adapter`:

```rust
pub enum PlanStep {
    Hook   { path: PathBuf },              // hook.sh / synthesize_diff.sh
    Plugin { path: PathBuf },              // hector.ts
    Patch  { path: PathBuf, key: &'static str },  // settings.json › PostToolUse
    Skill  { path: PathBuf },              // SKILL.md
}
```

- Add dedicated planners: `plan_install(h, env, scope) -> Vec<PlanStep>` and
  `plan_uninstall(h, env, scope) -> Vec<PlanStep>`. These lift the path
  computation out of the current `dry_run` branches in `ops.rs` — the branches
  that today build `format!("write {}", ...)` strings become these functions,
  returning structure.
- The flow is now two explicit calls: `plan_install(…)` to preview, then (on
  confirm) `install(…)` with `dry_run = false` for real. This mirrors today's
  single-call `if dry_run { … } else { … }` split, just with the preview half
  hoisted into a named function whose output the CLI can render however it likes.
- The `InstallResult::DryRun(Vec<String>)` variant is **removed** entirely, and
  with it the `DryRun` match arms in `onboard.rs`'s `format_outcome` /
  `format_skill_outcome`. Dry-run is no longer an `InstallResult` — it is
  "render the plan, then stop." No caller outside the adapter + CLI consumes it.
- Uninstall uses `plan_uninstall`, whose `PlanStep`s the CLI renders with a
  "remove" verb (a removal plan).

**Boundary:** core produces data (`PlanStep`), CLI produces pixels. The
pretty-printer lives entirely in `hector-cli` and is unit-tested as pure string
formatting, exactly like today's `format_outcome`.

**Skill dedup is honored in the plan.** `should_install_skill` already skips
opencode's skill when claude-code is in the same set (they share
`.claude/skills/`). The plan must reflect reality: when both are selected, omit
opencode's `Skill` step so the preview matches what actually gets written.

### 2. Unified onboarding flow (CLI)

`run_hook_phase` collapses to one pipeline for both entry paths:

1. **Resolve harness set + source tag.** `--harness pi` → `pi (requested)`;
   auto-detect → `pi (detected)`; `--harness all` → every harness `(requested)`.
2. **Build the plan** (`Vec<(Harness, Source, Vec<PlanStep>)>`).
3. **Render the plan** (the tree; see §3).
4. **Confirm** — `Proceed? [Y/n]`, default yes; skipped under `--yes`.
5. **Install**, then print the concise per-harness result line
   (`installed — <restart hint>`, `already present`, `updated`, …) — unchanged
   from today.

This removes the current fork where explicit installs silently and only detect
prompts.

**Edge cases** (each follows today's behavior where one exists):

| Situation | Behavior |
|---|---|
| `--yes` | render plan, skip prompt, install |
| `--dry-run` | render plan, then **stop** — the plan view *is* the dry run |
| non-TTY + explicit `--harness` | render plan, install (can't prompt; intent already named) |
| non-TTY + auto-detect, no `--yes` | render plan, print "re-run with `--yes` or `--harness <name>`", install nothing (matches today) |
| nothing detected | today's "no supported harnesses detected…" message, unchanged |
| `--uninstall` | render a **removal** plan, confirm, uninstall |
| `--no-hook` | scaffold config only; onboarding phase not entered (unchanged) |

### 3. Rendering ("prettier")

Layout is a per-harness tree under a section header:

```
  hector · onboarding
  ───────────────────

  claude-code   detected
    ├ hook    ~/.config/hector/adapters/claude-code/hook.sh
    ├ hook    ~/.config/hector/adapters/claude-code/synthesize_diff.sh
    ├ patch   ./.claude/settings.json  › PostToolUse
    └ skill   ./.claude/skills/hector-config/SKILL.md

  pi            requested
    ├ plugin  ./.pi/extensions/hector.ts
    └ skill   ./.pi/skills/hector-config/SKILL.md

  Proceed? [Y/n]
```

Rules:

- **Section header** `hector · onboarding` with an underline rule.
- **Step-kind column** (`hook`/`plugin`/`patch`/`skill`) is fixed-width so paths
  align. Patch shows its array key after `›`.
- **Path display** is shortened: home-relative (`~/…`) for artifacts under the
  home/config dir, project-relative (`./…`) for files under the project root.
  Avoids leaking the absolute home path and keeps lines short. Fallback to the
  absolute path when neither prefix applies.
- **Color** via a tiny internal ANSI helper (~15 lines, no new dependency),
  gated on `std::io::stdout().is_terminal()`: bold harness name, dim paths,
  colored source tag. Piped / non-TTY output is plain text — the exact same tree
  with no escape codes (matters for CI capture and `--dry-run | tee`). No
  reliance on ratatui/crossterm, which are the TUI's concern.

The plan describes **what the install touches**, not a diff against current
on-disk state. Actual outcome (installed / already-present / updated) is reported
per-harness *after* install by the existing result lines.

## Testing

- **Core:** `plan_install`/`plan_uninstall` return the expected `Vec<PlanStep>`
  per harness kind (JsonHook two files + patch; Plugin one file; skill step;
  opencode-skill omitted when claude-code present). Planners write nothing —
  assert the target files/dirs do not exist after a `plan_*` call (replaces the
  old `dry_run` "writes nothing" assertions, which move here since the `DryRun`
  variant is gone).
- **CLI (pure formatting):** the plan renderer is tested as a pure function over
  `(Harness, Source, Vec<PlanStep>)` → lines, covering: tag rendering
  (detected/requested), column alignment, patch-key suffix, path shortening
  (home/project/absolute fallback), and the no-color branch (assert no `\x1b`
  when not a terminal). Mirrors how `format_outcome` is tested.
- **Flow:** confirm default-yes, `--yes` skips prompt, `--dry-run` renders and
  stops (installs nothing), non-TTY branches per the edge-case table.
- Existing `format_outcome` / result-line tests stay; the post-install output is
  unchanged.

## Region-coverage / complexity notes

- New CLI formatter is small pure functions — easy to hold ≥80% region coverage.
- Keep the render pipeline decomposed (tag → header → per-harness block → step
  line) to stay under the cognitive-complexity cap of 15 per function.
