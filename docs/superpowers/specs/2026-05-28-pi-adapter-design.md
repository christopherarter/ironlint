# Hector — pi adapter (tool_call gate)

**Status:** Designed 2026-05-28. Not yet scaffolded.
**Date:** 2026-05-28
**Owner:** dynamik-dev
**Companion to:** [`overview.md`](../../../specs/overview.md), [`2026-05-25-reasonix-adapter.md`](../../../specs/2026-05-25-reasonix-adapter.md), [`adapters/opencode/`](../../../adapters/opencode/)
**Scaffold (to build):** `adapters/pi/` — a pi extension wiring `tool_call` / `tool_result` / `session_start` / `agent_end` to the `hector` CLI.

---

## 1. Summary

[pi](https://pi.dev) is an open-source terminal coding agent (`@earendil-works/pi-coding-agent`, repo `earendil-works/pi`). It exposes an **extension** system: TypeScript modules, loaded via `jiti` (no build step), that subscribe to lifecycle events and can **block tool calls before they execute**.

This is the cleanest integration surface Hector has targeted yet. pi's `tool_call` event fires *before* the tool runs and blocks by returning `{ block: true, reason }` — no exception-throwing (opencode) or exit-code-2-from-a-shell-hook (reasonix/claude-code) gymnastics. The adapter is a pure translation layer between pi's lifecycle and the `hector` binary; it contains no rule logic.

The design mirrors the **opencode** adapter feature-for-feature (per-edit gate + session record + session check + stale-state clear), adapted to pi's API and using the `--content -` pre-write check pioneered by the **reasonix** adapter.

## 2. pi protocol (verified)

Verified against `earendil-works/pi` (`packages/coding-agent/src/core/tools/`) and `pi.dev/docs/latest/extensions`.

### 2.1 Extension shape

Default-exported factory, sync or async:

```typescript
export default function (pi: ExtensionAPI) {
  pi.on("tool_call", async (event, ctx) => { /* ... */ });
}
```

Runtime: TypeScript via `jiti`. Node built-ins (`node:fs`, `node:child_process`) and npm deps (from a local `package.json`) are available. Type imports come from `@earendil-works/pi-coding-agent`.

### 2.2 Discovery / install paths

- Global: `~/.pi/agent/extensions/*.ts` or `~/.pi/agent/extensions/*/index.ts`
- Project: `.pi/extensions/*.ts` or `.pi/extensions/*/index.ts`
- `settings.json` → `"extensions": ["/abs/path/index.ts", …]`
- npm package → `package.json` `"pi": { "extensions": ["./src/index.ts"] }`
- Ad-hoc load: `pi -e ./adapters/pi/src/index.ts`; hot-reload with `/reload`

### 2.3 Lifecycle events used

| Event | Fires | Can block? | Payload (fields we read) |
|---|---|---|---|
| `tool_call` | before a tool executes | **yes** | `{ toolName, toolCallId, input }` — `input` is mutable |
| `tool_result` | after a tool finishes | modifies result only | `{ toolName, input, content, details, isError }` |
| `session_start` | session begins | n/a | — |
| `agent_end` | after each user-prompt turn | no (turn already done) | — |

To block at `tool_call`: `return { block: true, reason: "…" }` (the `reason` is optional). Returning `undefined`/nothing allows the tool. Handlers may be async.

Other available events not used in v1: `before_agent_start`, `agent_start`, `input`, `context`, `tool_execution_{start,update,end}`, `session_shutdown`. We choose `agent_end` over `session_shutdown` for the session check because it is the per-turn analog of opencode's `session.idle`; `session_shutdown` only fires once at teardown.

### 2.4 Built-in file tools (the gate targets)

pi gives the model `read`, `write`, `edit`, `bash` (plus `grep`, `find`, `ls`). The two we gate:

- **`write`** — schema `{ path: string, content: string }`. `content` is the full post-write file body. The renderer also tolerates a `file_path` alias.
- **`edit`** — schema `{ path: string, edits: Array<{ oldText: string, newText: string }> }`. A **batch** of exact-text replacements applied in sequence; each `oldText` must be unique in the file. A **legacy** top-level `{ path, oldText, newText }` form is normalized by pi into a single-element `edits[]` — the adapter must accept it too. `file_path` tolerated as a `path` alias. When an exact `oldText` match fails, pi falls back to a **fuzzy match** (`fuzzyFindText`: trims trailing whitespace, normalizes Unicode quotes/dashes).

### 2.5 Child-process execution

pi documents `pi.exec(command, args, options?)` with a `signal` option for abort-awareness. **Open question (§14):** whether `pi.exec` accepts stdin input and surfaces an exit code. The adapter needs both (proposed content on stdin, exit code for the contract). If `pi.exec` does not support stdin, fall back to `node:child_process` (`execFileSync`/`spawnSync` with `input`).

## 3. Design decisions

| Decision | Choice | Rationale |
|---|---|---|
| Feature scope | **Full opencode parity** | pi's lifecycle supports the full set at low marginal cost. |
| Proposed-content delivery | **`--content -` via stdin** | No disk mutation; AST/semantic/`hector-disable` rules gate correctly pre-write. Tradeoff: `engine: script` rules read the pre-edit on-disk file (same documented limitation as reasonix). |
| Test harness | **`node:test`** | Matches pi's Node/jiti runtime; zero extra deps. |
| Session-check trigger | **`agent_end`** | Per-turn analog of opencode's `session.idle`. |
| `edit` `oldText` unmatchable | **Skip the gate** (fail-open on simulate-failure) | pi's fuzzy fallback may still apply the edit; blocking on our simulation miss would be a false positive. Matches opencode. |
| Package name | **`@dynamik-dev/hector-pi`** | Matches `@dynamik-dev/hector-opencode`. |

## 4. Lifecycle mapping

| pi event | Action |
|---|---|
| `tool_call` (`write`/`edit`) | Compute proposed content → `hector check --file <path> --content - --config <cfg> --format json` → `return { block: true, reason }` on exit 2. |
| `tool_result` (`write`/`edit`) | `hector session record --dir <root> --file <path> --diff <synthetic-diff>` (best-effort). |
| `session_start` | Delete stale `.hector/session.json`. |
| `agent_end` | If `.hector/session.json` exists, `hector check --session`. Advisory — the turn is over, so surface the verdict (cannot retroactively block). |

## 5. The gate (`tool_call`)

```
1. configPath = <root>/.hector.yml
2. if !exists(configPath) → return            # re-checked every call: mid-session
                                               #   `hector init` starts gating
3. if toolName ∉ {write, edit} → return
4. path = input.path ?? input.file_path
   if !path → return
5. if basename(path) ∈ {.hector.yml, .bully.yml} → return   # R3 self-edit short-circuit
6. proposed = computeProposedContent(toolName, path, input)
   if proposed === null → return              # can't faithfully simulate; let pi's
                                               #   fuzzy edit proceed ungated
7. result = run hector check --file <path> --content - --config <cfg> --format json
            (proposed piped on stdin)
8. map result.exitCode:
     0       → return                          # pass/warn → allow
     2       → return { block: true, reason: <stdout verdict || "rule violation"> }
     3       → fail-open: console.error(stderr); return
                unless HECTOR_FAIL_CLOSED_ON_INTERNAL=1 → { block: true, reason }
     other   → console.error("internal error … exit N"); return   # fail-open
```

### 5.1 `computeProposedContent(toolName, path, input)`

- **`write`** → `input.content` (the full body), even if the file is new. If `content` is absent or not a string (malformed call) → `null` (skip; pi would reject the call anyway).
- **`edit`** → normalize input to an `edits: [{oldText,newText}]` array (accept the legacy top-level `{oldText,newText}`). If the file doesn't exist → `null`. Read current content; for each edit in order, require `oldText` to occur exactly once in the working buffer, then replace its first occurrence. If any `oldText` is missing or non-unique → `null` (skip the gate). Return the final buffer.

Exact-match-and-unique mirrors pi's own contract. We deliberately do **not** attempt to reproduce pi's fuzzy fallback — divergence there would feed `hector` content pi won't actually write, risking false blocks; skipping is safer.

### 5.2 Exit-code contract

Identical to every Hector adapter (`commands/check.rs`): `0` pass/warn → allow · `2` block · `3` engine-internal error → fail-open by default, fail-closed under `HECTOR_FAIL_CLOSED_ON_INTERNAL=1` · `1`/other config error → log + allow.

## 6. session record (`tool_result`)

```
1. if !exists(configPath) → return
2. if toolName ∉ {write, edit} → return
3. if event.isError → return                   # the edit failed; nothing landed
4. path = input.path ?? input.file_path; if !path → return
5. if basename(path) ∈ {.hector.yml, .bully.yml} → return
6. diff = synthesizeDiff(path, toolName, input)
7. run hector session record --dir <root> --file <path> --diff <diff>   # errors swallowed
```

### 6.1 `synthesizeDiff` — port opencode's hardening

pi's tool events carry no real unified diff, so we fabricate one. Two correctness concerns carried over verbatim from the opencode adapter:

- **Hunk-header counts (P1-8):** emit `1,N` form whenever a side has >1 line, so `hector`'s diff parser numbers added lines correctly.
- **Injection scrub (P1-9):** any line in the user-controlled old/new blocks matching `^(---|\+\+\+|@@) ` gets a leading backslash, so a malicious `newText` containing a fake `+++ b/SECRET` header cannot redirect the parser to another file.

pi's `edit` is a batch, so `synthesizeDiff` emits **one scrubbed hunk per `{oldText,newText}`** (a `write` is the single-hunk `"" → content` case).

## 7. session check (`agent_end`)

```
1. if !exists(.hector/session.json) → return
2. result = run hector check --session --config <cfg> --format json
3. map exitCode:
     2     → console.error / ctx.ui.notify(verdict)   # advisory: turn already finished
     3     → fail-open (or closed under env)
     other → log
```

`agent_end` fires after the agent's turn completes, so — like opencode's `session.idle` — the check cannot retroactively block. It surfaces the verdict so the user (and the next turn) see what to fix.

## 8. session_start

Delete a stale `.hector/session.json` left by a prior aborted run (`rmSync(..., { force: true })`), best-effort.

## 9. Error handling & invariants

- **Fail-open** on internal/config errors (exit 1/3) by default; `HECTOR_FAIL_CLOSED_ON_INTERNAL=1` flips exit-3 to a block. A misconfigured `hector` must never brick the agent.
- **Best-effort** for `session record` and stale-state cleanup — a flaky write there never affects the agent.
- **R3 self-edit short-circuit:** edits to `.hector.yml` / `.bully.yml` skip both the gate and session record (a mid-edit policy file fails the trust gate and would surface a confusing internal error).
- **Late config re-check:** every handler re-checks `.hector.yml` existence, so installing the extension globally is safe and mid-session `hector init` begins gating without a restart.

## 10. Files

```
adapters/pi/
  src/index.ts        # factory + helpers: computeProposedContent,
                      #   synthesizeDiff, exit-code mapper, runHector
  package.json        # @dynamik-dev/hector-pi, "pi": { "extensions": ["./src/index.ts"] }
  tsconfig.json
  test/index.test.ts  # node:test suite (drives the factory directly)
  README.md           # install, requirements, exit-code table, known gaps
```

Plus CI wiring in `.github/workflows/ci.yml` to run the node:test suite with the built `hector` on PATH (mirroring how the opencode bun-test suite is run; confirm the existing pattern during planning).

## 11. Testing (`node:test`)

Drive the exported factory with synthetic pi-shaped events against the **real** `hector` binary (CI prepends `target/release` to `PATH`). Cases mirror the opencode suite:

- no-op when `.hector.yml` absent
- gate activates after config is created mid-session (re-check regression)
- clean `write` passes; violating `write` returns `{ block: true }`
- `edit` introducing a violation blocks; on-disk file untouched (`--content -` never writes)
- **multi-edit batch** (`edits: [a, b]`) simulated correctly
- legacy single-edit `{ oldText, newText }` form
- `edit` whose `oldText` is missing / non-unique → gate skipped (no false block)
- non-gated tools (`read`, `bash`) ignored
- missing `path` → no-op
- `.hector.yml` / `.bully.yml` self-edit short-circuit (R3), incl. bare relative path
- `session_start` clears stale `session.json`
- `agent_end` no-ops without `session.json`; advisory surface on session block
- `tool_result` records the edit to `session.json`
- exit-3 fail-open by default; fail-closed under `HECTOR_FAIL_CLOSED_ON_INTERNAL=1`

## 12. Distribution / install

- **Local dev:** symlink or copy `src/index.ts` into `.pi/extensions/hector.ts` (project) or `~/.pi/agent/extensions/hector.ts` (global), or reference the absolute path in pi `settings.json` `"extensions"`. `pi -e ./adapters/pi/src/index.ts` for an ad-hoc load; `/reload` to hot-reload.
- **npm (once published):** `@dynamik-dev/hector-pi` with the `"pi": { "extensions": ["./src/index.ts"] }` field; pi installs deps on first load.
- Requires the `hector` binary on `PATH`. Silent no-op without `.hector.yml`, so global install is safe.
- Project setup: `hector init && hector trust`.

## 13. Known gaps (v1, documented in README)

- **`bash`-tool shell-out** (`cat > foo`, redirections) bypasses the gate — universal across all adapters; too brittle to parse arbitrary commands.
- **`edit` fuzzy-match fallback** can't be faithfully simulated → those edits skip the gate (fail-open on simulate-failure).
- **`engine: script` rules** read the pre-edit on-disk file under `--content -`; AST/semantic/`hector-disable` gate correctly pre-write.
- **pi subagents** not specially handled (claude-code ships a subagent mode; deferred).
- **`agent_end` session check is advisory** — it cannot retroactively block a finished turn.

## 14. Open implementation question

Confirm at the start of implementation whether pi's `pi.exec(command, args, options?)` supports (a) stdin input and (b) exit-code retrieval. If yes, use it (gains `ctx.signal` abort integration). If not, use `node:child_process` (`execFileSync`/`spawnSync` with `{ input, encoding }`) — deterministic stdin + exit code, at the cost of no abort wiring. Resolve by reading `pi.exec`'s signature in `earendil-works/pi` before writing `runHector`.
