# Hector 0.4 — checks, lifecycles, and the local-CI repositioning

**Status:** design, approved direction (2026-06-28)
**Builds on:** the 0.3 gates substrate (`specs/2026-06-15-hector-gates-redesign-design.md`)
**Revises** (not supersedes wholesale) specific 0.3 decisions: §2 config language, §3 verdict contract, §4 ABI, §5 execution model, §9 verdict JSON, §10 telemetry.
**Breaking:** config + verdict-JSON shape change. No migration path — hector is not yet distributed. Hector's own `.hector.yml` and the e2e fixtures move with it.

## 1. Thesis — same substrate, a legible frame

The 0.3 substrate is right. The *framing* is the problem. "Static policy-enforcement gate for AI coding agents" is accurate and inert; nobody pictures it. **"A local CI for agents"** is lossy but instantly legible: checks that run on your agent's edits and at commit, locally — and that can refuse. The analogy leaks in exactly one place, and it's the differentiator: CI reports *after* you push; hector intercepts *before* the write lands and blocks. The honest tagline is "local CI that can actually stop the edit."

This phase repositions the same substrate behind that mental model and makes the config **read like a GitHub Actions workflow** — without importing Actions' complexity and without adding a check taxonomy. The 0.3 thesis is unchanged and is what makes the reframe honest: hector owns the portable plumbing (trigger + ABI + verdict wire) and **knows nothing about any tool**. "Give the agent the plumbing" is the whole product. The scripts own tool behavior; hector owns the guarantees.

**Lefthook absorption (positioning, §8).** At the commit boundary hector is a near-isomorphic swap for lefthook's *gate* role (`glob`→`files`, `{staged_files}`→`$HECTOR_FILES`, `run`→`run`). It deliberately declines lefthook's *fixer* role (`stage_fixed`, parallel mutate-and-restage) and adds the `write` lifecycle lefthook structurally cannot reach — interception before the file is on disk. You absorb the gate, decline the task-runner, and gain the pre-write gate.

## 2. What changes vs 0.3 (the diff)

| 0.3 | 0.4 | revises |
| --- | --- | --- |
| `gates:` map | `checks:` map | §2 |
| gate = `{ files, run }` | check = `{ files, run \| steps, on?, name? }` | §2 |
| exit `2` blocks; `1` is a pass | exit `1`–`125` blocks; broken-gate tier kept | §3 |
| `$HECTOR_EVENT ∈ {edit, write, pre-commit, manual}` | drop `edit` (folded into `write`); add `$HECTOR_FILES` | §4 |
| one `run` per matching file, always | `write` per-file; `pre-commit` runs **once** over the set | §5 |
| verdict JSON schema 4, keys `gate` | schema **5**, keys `check`, optional `step` | §9 |

Everything else from 0.3 — `extends` (cycle-checked DFS, local wins), the out-of-repo trust store, `execution.timeout_secs` + `HECTOR_TIMEOUT`, scope semantics (`config/scope.rs`, bare-pattern-matches-any-depth), inline disable, `hector verify`/`doctor` direction, the **outer** exit-code contract — is unchanged.

## 3. The config language (the whole thing)

```yaml
# .hector.yml — checks that run on your agent's edits and at commit, locally.
extends: []                       # optional; inherited checks fill gaps, local wins
execution:
  timeout_secs: 30                # optional; per-step wall-clock

checks:
  # minimal: a file match + one command. a nonzero exit blocks.
  rustfmt:
    files: '**/*.rs'
    run: rustfmt --check "$HECTOR_FILE"

  # forbid a pattern: grep reads the proposed content on stdin; `!` turns a
  # match into a failure. no exit-2 ceremony.
  no-todo:
    files: 'crates/*/src/**/*.rs'
    run: '! grep -nE "todo!\("'

  # opt-in multi-step; first failing step blocks, its output is the message.
  typescript:
    name: TypeScript must be clean
    files: 'src/**/*.ts'
    on: [pre-commit]
    steps:
      - name: Compiles
        run: tsc --noEmit
      - name: No focused tests
        run: '! grep -n "\.only(" $HECTOR_FILES'

  # project analysis: runs once at the root, walks the tree itself. hector is
  # pure plumbing — it triggers, sets cwd + $HECTOR_FILES, reads the exit code.
  no-cycles:
    files: 'src/**/*.ts'
    on: [pre-commit]
    run: depcruise src --validate
```

A **check** is a file match plus work to run:

- `files` — one glob or a list. **Required.** Scope semantics unchanged from 0.3 (a bare pattern without `/` also matches at any depth, so `*.py` ≡ `**/*.py`). `files` is the *trigger filter*: a check wakes when a touched file matches. On `write` the matched file is *the* file; on `pre-commit` the matched files are the set. It does not, by itself, fan a project tool out over every file (§5).
- `run` **xor** `steps` — the work. `run: X` is exactly `steps: [{ run: X }]`; it is the one-step shorthand. A check with **both** or **neither** is a config error.
- `steps` — an ordered list of `{ name?, run }`. Opt-in; the common case stays a one-line `run`. Fail-fast (§5).
- `on` — the lifecycle(s) the check fires in: any of `write`, `pre-commit`. **Optional, default `[write]`.** This is the one genuinely new *capability*: event-scoped checks (fast greps on every edit, slow project checks at commit). `on` is a per-check list — the same check piped into multiple lifecycles with no duplication, which inverts lefthook's hook-keyed layout (§8).
- `name` — optional human label for output (`✗ typescript › Compiles`). The **map key remains the stable id** for `--check`, `hector-disable`, and telemetry; `name` is cosmetic.

**Deliberately not added** (the scope line — this keeps the 0.3 "no taxonomy of check kinds" thesis intact): no `uses:`/reusable actions, no `with:`, no `scope: file|project` field, no `severity`, no output parsing, no `parallel`/`stage_fixed` (lefthook's fixer half). A check's *shape* is carried by its lifecycle and its command, never by a new config knob.

## 4. The verdict contract, revised — nonzero blocks

Hector runs each step and reads **only its exit code**. The block threshold moves from "exactly `2`" to "any ordinary failure":

| step exit code            | meaning                       | contributes |
| ------------------------- | ----------------------------- | ----------- |
| `0`                       | pass — continue to next step  | —           |
| `1`–`125`                 | **Block**                     | Block       |
| `126`, `127`              | not executable / not found    | InternalError |
| `≥128` (killed by signal) | step crashed                  | InternalError |
| wall-clock timeout        | step hung                     | InternalError |

**Why the flip.** "Exit 2 to count" was the single most surprising thing for a CI-shaped mental model, and it forced `… || exit 2` / `grep -q X && exit 2` ceremony onto every gate. Under nonzero-blocks, the entire tool-check category just works with zero idiom — `rustfmt --check`, `tsc --noEmit`, `eslint`, `phpstan`, `prettier --check` all already exit nonzero on findings, so they block for free. This is the CI/lefthook/Make convention ("nonzero failed the build") and it is what makes the "local CI" frame click.

**The trade, stated honestly.** 0.3's flexibility was opt-*in* blocking (a tool exits 1 on findings but you choose not to block). 0.4 inverts it to opt-*out*: a failing command blocks unless you neutralize it with `|| true`. That is the correct default for a gate.

**The one new idiom — forbid.** `grep` exits `0` when it *finds* the thing, which is backwards for a gate, so prefix `!` to turn a match into a failure: `! grep -nE "todo!\("`. The match lines grep prints (with `-n`) become the block message. For a custom message: `grep -q X && { echo "msg" >&2; exit 1; }`.

**Broken gate is never a silent block.** The 0.3 carve-out is preserved precisely so a typo cannot wedge the agent: `126`/`127`/timeout/signal map to **InternalError**, not Block. So "nonzero blocks" means specifically `1`–`125`.

**Output.** On Block, the failing step's combined stdout+stderr passes through verbatim as the message; empty → `"<check-id> blocked"` (or `"<check-id> › <step name> blocked"` when the step is named).

**Outer (hector's own) exit codes — unchanged; adapters and CI depend on them.** `0` pass · `1` config/load error (incl. untrusted) · `2` Block (≥1 check blocked) · `3` InternalError (≥1 step crashed; adapters fail-open, `HECTOR_FAIL_CLOSED_ON_INTERNAL=1` flips). Only a *step's internal* classification changed; the surface CI/adapters consume did not.

## 5. Steps & execution — fail-fast, lifecycle picks the shape

**Steps run in order; the first to exit `1`–`125` blocks, fail-fast.** That step's output is the message; remaining steps are skipped. All steps `0` → the check passes. A `126`/`127`/timeout/signal on any step is InternalError against that step (fail-open). Each step is an independent `sh -c <run>` with the full ABI; steps do **not** share shell state (separate processes) — they share cwd, env, and the stdin content (re-fed fresh per step on `write`). Steps are for *checking*, not for building state; this is what keeps hector a gate, not a task runner. `execution.timeout_secs` (and `HECTOR_TIMEOUT`) bound **each step**.

**The lifecycle determines the execution shape**, because a write and a commit are genuinely different events:

- **`write`** — an agent edits or creates one file. Exactly one file; its proposed content arrives on **stdin** (the file may not be on disk yet — this is hector's pre-landing superpower). Per-file. Content guards live here.
- **`pre-commit`** — a commit touches a *set*. The check runs **once** at the project root; the changed set is in **`$HECTOR_FILES`** (on disk, staged). The command owns its scope: project tools (`depcruise`, `tsc`, the test suite) ignore the set and walk the tree; file tools take the set as args (`prettier --check $HECTOR_FILES`). This is lefthook's proven "glob triggers, run consumes `{staged_files}`" model, and it is why a whole-project check is no longer "redundant but correct" per-file (0.3 §5) — it runs once.

**The seam, documented.** A single `run` string is not guaranteed to work identically in both lifecycles: on `write` the content is on **stdin**; at `pre-commit` it is on **disk** via `$HECTOR_FILES`. That seam is intrinsic to the two events. Guidance: **put each check where its shape fits** — content guards → `write`, project analyses → `pre-commit`. The genuinely-both case (a tool that accepts stdin *and* file args) exists but is the exception, not the rule.

## 6. The ABI (locked stability surface, lifecycle-aware)

| channel         | `write`                                  | `pre-commit`                              |
| --------------- | ---------------------------------------- | ----------------------------------------- |
| `$HECTOR_FILE`  | absolute path of the one file            | *unset* (no single file)                  |
| `$HECTOR_FILES` | that one file (so file-arg commands port)| newline-delimited changed set (on disk)   |
| `$HECTOR_ROOT`  | project root                             | project root                              |
| `$HECTOR_EVENT` | `write`                                  | `pre-commit`                              |
| **stdin**       | proposed post-edit content (may be empty)| *empty* (read from disk)                  |
| **cwd**         | `$HECTOR_ROOT`                           | `$HECTOR_ROOT`                            |

`write` covers both editing an existing file and creating a new one (0.3's `edit` and `write` collapse — they were the same for our purposes). `$HECTOR_FILES` is newline-delimited; a NUL-delimited variant for pathological filenames is a later refinement, not a 0.4 blocker. The path travels only as an env value, never spliced into command text.

**`hector check` (manual / CI entrypoint).** `--event` selects the lifecycle to simulate (default `pre-commit` for the CI use case; `--file`/stdin simulates a `write`). Manual runs bypass the `on:` filter so any check can be invoked on demand; `$HECTOR_EVENT=manual`. Otherwise it uses the simulated lifecycle's ABI and dispatch shape.

## 7. Verdict JSON, telemetry, disable

**Verdict JSON — `SCHEMA_VERSION` 4 → 5.** Keys rename to the check/step vocabulary; a `step` field is added when a multi-step check is the source. Treat as a locked surface.

```json
{
  "schema": 5,
  "status": "pass" | "block" | "internal_error",
  "blocks": [
    { "check": "typescript", "step": "Compiles", "file": "src/x.ts", "message": "<verbatim stdout+stderr>" }
  ],
  "errors": [
    { "check": "no-cycles", "step": null, "file": null, "reason": "timeout" | "not_found" | "signal:9" }
  ]
}
```

`file`/`step` are `null` where they do not apply (a `pre-commit` run-once check has no single file; a single-`run` check has no step name). Telemetry (`telemetry::SCHEMA_VERSION`, versioned independently) retargets to the same vocabulary: one record per check invocation with `check`, `event`, `exit_code`, `verdict`, `duration_ms`, optional `step`; `file` present on `write`, absent (with the set's size) at `pre-commit`.

**Inline disable** is unchanged in mechanism, renamed in vocabulary: `hector-disable: <check-id>` in a checked file suppresses that check for that file (file-wide; directive ends at whitespace/`*`/`/`).

## 8. Lefthook parity (positioning, captured because it drove the design)

```yaml
# lefthook.yml                          # .hector.yml
pre-commit:                             checks:
  commands:                               prettier:
    prettier:                               files: '**/*.{ts,css,md}'
      glob: "*.{ts,css,md}"                 on: [pre-commit]
      run: prettier --check {staged_files}  run: prettier --check $HECTOR_FILES
```

Near line-for-line. Mapping: `pre-commit:`→`on: [pre-commit]`, `commands.<id>`→`checks.<id>`, `glob:`→`files:`, `{staged_files}`→`$HECTOR_FILES`, `run:`→`run:`. **Absorbed:** the gate role. **Declined:** `parallel`, `stage_fixed`, `exclude`/`root`/`tags` — the fixer/task-runner half. **Added:** the `write` lifecycle — lefthook's earliest reach is `pre-commit`, after the agent already wrote the files; hector fires on the write itself. Structural inversion: lefthook keys by hook (duplicate a command to run it at two hooks); hector keys by check (`on: [write, pre-commit]`, no duplication).

## 9. Code impact (orientation, not a plan)

- **`config/types.rs`** — `Gate { files, run }` → `Check { files, run: Option, steps: Option<Vec<Step>>, on: Vec<Lifecycle> (default `[write]`), name: Option }`; `Step { name: Option, run }`; `Config.gates` → `Config.checks`. Validate `run` xor `steps` (serde + a post-parse check, like the existing empty-`run` guard). `Lifecycle` enum = `Write | PreCommit`.
- **`config/parser.rs`** — legacy rejection grows a curated marker: top-level `gates:` → "renamed to `checks:`". Keep the `schema_version`/`rules`/`trust` rejections. Keep the empty/comment-only `run` guard; apply it per step.
- **`engine/gate.rs`** — classification flip (`1`–`125` → Block; `0` → Pass; `126`/`127`/`≥128`/timeout → InternalError); a steps loop with fail-fast and per-step timeout; message assembly carries the step name.
- **`runner.rs`** — dispatch keys on lifecycle: `write` = per matching file with stdin + `$HECTOR_FILE`/`$HECTOR_FILES`; `pre-commit` = run-once over the matched set with `$HECTOR_FILES`, no `$HECTOR_FILE`, empty stdin. Honor the `on:` filter (skip checks whose `on` excludes the event); manual bypasses it.
- **`verdict.rs`** — schema 5, `check`/`step` keys, nullable `file`/`step`.
- **`telemetry.rs`** — retarget keys/vocabulary; per-invocation record.
- **adapters** — emit `$HECTOR_EVENT=write` (folding the old `edit`) and `$HECTOR_FILES`; the pre-commit adapter passes the staged set. (The full adapter `--event`/ABI side remains Plan 4 territory; 0.4 locks the contract they target.)
- **repo's own config + fixtures** — migrate `.hector.yml` and `tests/e2e/**` from `gates:` to `checks:`; update CLI/E2E assertions keyed on `gate`/exit-2.
- **docs / README / `hector-config` skill / `hector schema`** — reposition to "local CI for agents," the checks vocabulary, nonzero-blocks, and the `write`/`pre-commit` ABI split.

## 10. Testing

- **Unit (classification):** `0` pass · `1`/`2`/`125` block · `126`/`127`/`signal`/`timeout` InternalError — the flip is the highest-value regression to pin.
- **Unit (steps):** fail-fast stops at the first failing step; message = that step's output; all-pass passes; per-step timeout; `run` xor `steps` (both → error, neither → error).
- **Unit (config):** `on` defaults to `[write]`; `gates:` yields the curated "renamed to `checks:`" error; `name` is optional and does not affect the id.
- **Unit (ABI):** `$HECTOR_FILES` set in both lifecycles; `$HECTOR_FILE`/stdin only on `write`; `pre-commit` runs once over a multi-file set.
- **E2E (`assert_cmd`):** `write` per-file stdin gating; `pre-commit` run-once with `$HECTOR_FILES` (a project-shape check that walks a tree, and a file-arg check that consumes the set); lefthook-parity `prettier`; nonzero-blocks for a real tool (`rustfmt --check`) with no remap.
- Coverage gate (≥80% region per touched file) and the cognitive-complexity cap (15) still apply; the steps loop is the likely complexity pressure point — decompose rather than annotate.

## 11. Out of scope (YAGNI)

Reusable actions (`uses:`/`with:`), a `scope:` field, parallel dispatch, `stage_fixed`-style auto-staging, output parsing, per-tool modeling. Deferred but compatible (no shape change): **report-all** steps (run every step, collect all failures) as an opt-in alternative to fail-fast; a NUL-delimited `$HECTOR_FILES`; per-step `continue-on-error`. Batch de-duplication remains out (the `pre-commit` run-once model removes the original motivation).

## 12. Resolved decisions

1. **Block threshold** — exit `1`–`125` blocks; `0` passes; `126`/`127`/signal/timeout = InternalError (broken-gate fail-open preserved). Reverses 0.3 §3's exit-2 opt-in.
2. **Steps** — fail-fast; first failing step is the message. `run` is one-step sugar; `run` xor `steps`.
3. **`on:` default** — `[write]`. `write` covers editing and new files (folds 0.3's `edit`).
4. **`pre-commit` shape** — run **once** over the changed set via `$HECTOR_FILES`; not per-file. Resolves the per-set question deferred in 0.3 §5.
5. **ABI** — add `$HECTOR_FILES` (both lifecycles); `$HECTOR_FILE`/stdin are `write`-side; drop `edit` from `$HECTOR_EVENT`.
6. **No new first-class config elements** — `uses:` explicitly declined; a check's shape is its lifecycle + command, not a knob.
7. **Vocabulary** — `gates:`→`checks:`, `gate`→`check` everywhere (config, JSON, telemetry, disable, CLI flags), in service of the "local CI for agents" repositioning.
