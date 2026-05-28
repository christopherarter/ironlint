# Hector — adapter-drift-audit skill (harness contract maintenance)

**Status:** Designed 2026-05-28. Not yet scaffolded.
**Date:** 2026-05-28
**Owner:** dynamik-dev
**Companion to:** [`overview.md`](../../../specs/overview.md), the shipped adapters under [`adapters/`](../../../adapters/), and the [pi-adapter design](2026-05-28-pi-adapter-design.md).
**Scaffold (to build):** `.claude/skills/adapter-drift-audit/` — a repo-local, maintainer-facing skill that loads a harness's contract intel into context and audits the corresponding Hector adapter for drift.

---

## 1. Summary

Each Hector adapter (`adapters/claude-code/`, `adapters/opencode/`, `adapters/reasonix/`, and the designed `adapters/pi/`) is a thin translation layer between a coding harness's lifecycle/plugin contract and the `hector` CLI. Those harness contracts move — hook payload field names change, new lifecycle events appear, plugin manifest schemas gain fields, new file-mutating tools ship. When that happens, an adapter silently rots: it keeps running but stops gating what it used to gate.

`adapter-drift-audit` is a **maintainer tool** that closes that gap. Invoked with a harness name, it:

1. Loads that harness's **intel** — thesis, authoritative doc sources, and a contract surface map tying each consumed contract to the exact adapter file that consumes it — into context.
2. Fetches the harness's **current truth** from those sources (Context7, GitHub changelog, web docs).
3. **Compares** current truth against what the adapter actually does and emits a **drift report**.

It is **read-only**: it produces findings and recommendations but applies no edits to the adapter, and it does not audit `hector` core. The maintainer reads the report and decides what to change.

## 2. Audience and placement

| Decision | Choice | Rationale |
|---|---|---|
| Audience | **Hector maintainers** (you / dynamik-dev) | Keeping adapters in sync with harnesses is a maintenance concern, not a consumer concern. |
| Location | **`.claude/skills/adapter-drift-audit/`** (repo-local) | Versioned alongside the adapter code it audits; a contract-map row points at a real `file:line` in this repo. A global or plugin-bundled skill would decouple the map from the code. |
| Behavior | **Load intel → audit → report** (read-only) | The maintainer decides fixes; the skill never edits adapter files. Keeps the tool safe to run anytime. |
| Structure | **Shared procedure + per-harness reference files** | One audit procedure, written once; adding a harness = adding one reference file. Four harnesses already exist, so the abstraction pays for itself immediately. |

This skill is distinct from — and does **not** replace — the consumer-facing skills shipped *inside* each adapter (e.g. `adapters/claude-code/skills/{hector,hector-init,hector-author,hector-review}`). Those help an adapter's users author and run policy; this audits the adapter itself.

## 3. Layout

```
.claude/skills/adapter-drift-audit/
  SKILL.md              # shared audit procedure + harness index + report format
  references/
    claude-code.md      # built now (this spec)
    # pi.md / opencode.md / reasonix.md — added later, same template
```

"A skill for each adapter" is realized as **one reference file per adapter** under a single shared-procedure skill. The harness is selected by the skill's invocation argument.

## 4. SKILL.md — the shared procedure

### 4.1 Frontmatter

```yaml
---
name: adapter-drift-audit
description: Use when checking whether a Hector adapter still matches its coding
  harness's current contract — auditing adapter/harness drift, verifying hook
  payload shapes, plugin manifest schemas, lifecycle events, or tool names are up
  to date, or doing periodic adapter maintenance. Takes a harness name
  (claude-code, pi, opencode, reasonix) as argument.
---
```

### 4.2 Procedure

0. **Resolve target.** Read the invocation argument as the harness name and load `references/<harness>.md`. If no argument is given, list the harnesses that have a reference file and stop.
1. **Read the watermark.** The reference's `Last verified` line gives the baseline version / changelog date. Note it; it scopes step 2's changelog read.
2. **Fetch current truth.** For each entry in the reference's *Doc sources* section, in this order:
   - **Context7 first** (per repo + global convention): `resolve-library-id` is unnecessary — the reference pins exact IDs. `query-docs` each pinned ID for the contracts under audit.
   - **GitHub `CHANGELOG.md`** since the watermark — the primary signal for *what changed*.
   - **Web docs** as fallback / cross-check for any contract Context7 didn't cover.
3. **Compare each contract.** For every row in the reference's *Contract surface map*: read the cited adapter `file:line`, compare it to the fetched truth, and classify:
   - ✅ **in-sync** — adapter matches current contract.
   - ⚠️ **drift** — contract changed; adapter is stale.
   - ❓ **unverifiable** — couldn't fetch authoritative truth this run (note which source failed).
   - ✨ **new-capability-not-adopted** — harness now offers something relevant the adapter doesn't use (best-practice gap, not a bug).
4. **Emit the report** (fixed format, §4.3).
5. **Propose a watermark bump.** Print the suggested new `Last verified` line. The skill does **not** write it — keeping the read-only guarantee clean; the maintainer updates the reference when they act on the report.

### 4.3 Report format

```
# Adapter drift audit — <harness> (<date>)
Baseline: <watermark>   Current: <version/changelog date observed>

## Drift (⚠️)
- [contract #N: <name>] <adapter file:line>
  now: <current-truth summary>   (source: <doc link / Context7 id>)
  was: <what the adapter assumes>
  recommend: <concrete change>

## New capabilities not adopted (✨)
- <name> — <what it enables> (source: …) — optional; adapter still correct.

## Unverifiable (❓)
- [contract #N] <which source was unreachable>

## In sync (✅)
- <contract #N>, <contract #M>, …   (one line)

## Proposed watermark
Last verified: <date> against Claude Code <version> (changelog entry: <ref>)
```

## 5. references/claude-code.md — the intel (built now)

Five sections.

### 5.1 Thesis

Claude Code is Anthropic's agentic terminal coding tool. Its integration surface, as the adapter uses it:

- **Lifecycle hooks** fire on named events; a hook is a shell command registered in `hooks.json`. The adapter wires `PostToolUse` (gate each edit), `Stop` (session check), and `SessionStart` (clear stale state).
- **Hooks communicate by exit code + structured JSON stdout.** Exit `2` = block (stderr is fed back to the agent); exit `0` = allow. A hook can also print a `hookSpecificOutput` JSON object to inject `additionalContext` into the next turn — this is how subagent-mode defers semantic evaluation.
- **Plugins** bundle hooks, skills, agents, slash-commands, and MCP servers behind a `.claude-plugin/plugin.json` manifest. `CLAUDE_PLUGIN_ROOT` is injected at hook-fire time so commands resolve regardless of install path.
- **Two model paths**: direct-API (`llm.provider` is an API-keyed provider; the hook calls the LLM itself) vs. subscription subagent (`provider: claude-code-subagent`; the hook emits a deferred payload and a subagent evaluates it next turn, billing under the parent subscription).

An auditor should understand this shape so a drift finding can be judged for *impact*, not just *difference*.

### 5.2 Doc sources

| Source | Use for | Pointer |
|---|---|---|
| Context7 (primary) | Reference schemas: hooks, plugins, skills, sub-agents, settings | `/websites/code_claude` |
| Context7 (versioned) | Release notes pinned to a CLI version | `/anthropics/claude-code` (e.g. `v2.1.89`) |
| GitHub (drift signal) | `CHANGELOG.md` since the watermark; published plugin examples | `anthropics/claude-code` |
| Web (fallback) | Cross-check any contract Context7 missed | `docs.claude.com/en/docs/claude-code/{hooks,plugins,skills,sub-agents,settings,slash-commands}` |

Authority: for *schema shape*, prefer `/websites/code_claude` and the web hooks/plugins reference. For *what changed and when*, the GitHub `CHANGELOG.md` is canonical.

### 5.3 Contract surface map

Grounded in the adapter as it stands on 2026-05-28.

| # | Harness contract | Adapter consumer | Verify against |
|---|---|---|---|
| 1 | Hook event names `PostToolUse` / `SessionStart` / `Stop` | `hooks/hooks.json` | hooks reference |
| 2 | Matcher syntax + file-mutating tool set (`Edit\|Write`) | `hooks/hooks.json:5` | hooks reference + tool reference (watch for new mutating tools, e.g. `MultiEdit`/`NotebookEdit`) |
| 3 | `CLAUDE_PLUGIN_ROOT` injection at hook-fire time | `hooks/hooks.json:9,19,29` | hooks reference |
| 4 | **Hook stdin payload** — `tool_input.file_path` / `.path` / `.old_string` / `.new_string` / `.content` | `hooks/hook.sh:131,154-155` | hooks reference — **most drift-prone**; field renames silently break gating |
| 5 | Hook decision contract: exit `2` = block, `0` = allow | `hooks/hook.sh` (all case arms) | hooks reference |
| 6 | Output envelope `hookSpecificOutput.{hookEventName,additionalContext}` | `hooks/hook.sh:68-73,190-195` | hooks reference (`Stop` + `PostToolUse` JSON output) |
| 7 | Plugin manifest schema + location `.claude-plugin/plugin.json` | `.claude-plugin/plugin.json` | plugins reference |
| 8 | Skill `SKILL.md` frontmatter + description-based activation | `skills/*/SKILL.md` | skills reference |
| 9 | Subagent frontmatter (`name`/`description`/`model`/`tools`/`color`) | `agents/hector-evaluator.md:1-7` | sub-agents reference |
| 10 | Per-dispatch subagent model override (does inline override exist yet?) | `adapters/claude-code/README.md:55-65` (flagged unresolved) | sub-agents reference |

The `hook.sh` line numbers are anchors as of this spec; the procedure re-reads the file, so a shifted line still audits — the number just speeds location.

### 5.4 Known-fragile spots

Scrutinize these every run; they are the contracts most likely to have moved:

- **Hook stdin field names (#4).** `hook.sh` reads `.tool_input.file_path // .tool_input.path` and `.old_string // .new_string // .content`. A rename or restructure of the `PostToolUse` event payload breaks file extraction with no error — the hook just exits 0 and gates nothing.
- **File-mutating tool set (#2).** The matcher is `Edit|Write`. If Claude Code ships a new mutating tool, edits via that tool bypass the gate entirely.
- **Output envelope (#6).** Subagent mode depends on `hookSpecificOutput.additionalContext` being injected into the next turn. If the envelope key or injection semantics change, deferred semantic evaluation silently stops.
- **Per-dispatch model override (#10).** README:55-65 documents `evaluator_model` as *advisory only* because Claude Code's subagent dispatch did not accept a per-call model override at write time. If that capability ships, the skill should flag it as ✨ — the adapter can stop treating `evaluator_model` as a hint.

### 5.5 Watermark

```
Last verified: 2026-05-28 against Claude Code v2.1.89 (initial baseline — not yet audited)
```

The first real run replaces this with the version/changelog entry it observed.

## 6. Out of scope

- **Applying fixes.** Read-only by design; the report recommends, the maintainer edits.
- **Auditing `hector` core.** This skill audits adapter ↔ harness contract only.
- **Replacing consumer-facing adapter skills.** `hector`, `hector-init`, `hector-author`, `hector-review` remain the user-facing surface.
- **Other harness references.** `pi.md` / `opencode.md` / `reasonix.md` are deferred; each follows the §5 template when built.

## 7. Testing / validation

Skills aren't unit-tested like Rust code. Validate by:

1. **Dry run on a known-good adapter:** invoke `/adapter-drift-audit claude-code`; every contract-map row should resolve to a real `file:line` and either ✅ or a defensible ⚠️/❓. A row that can't find its anchor is a map bug.
2. **Synthetic-drift check:** temporarily rename a watched field (e.g. `.tool_input.file_path` → `.tool_input.path_x`) in a scratch copy and confirm the audit flags #4 as ⚠️. (Revert; do not commit.)
3. **Source reachability:** confirm each §5.2 pointer resolves — Context7 IDs return docs, the GitHub changelog is fetchable.

## 8. Open questions

None blocking. The §5.5 watermark is a placeholder until the first live run pins an observed version.
