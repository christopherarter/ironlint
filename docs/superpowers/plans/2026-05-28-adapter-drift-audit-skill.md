# adapter-drift-audit Skill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a repo-local, maintainer-facing skill that loads a coding harness's contract intel into context and audits the corresponding Hector adapter for drift (read-only report).

**Architecture:** A single skill, `adapter-drift-audit`, at `.claude/skills/adapter-drift-audit/`. `SKILL.md` holds the shared audit procedure and report format; `references/<harness>.md` holds per-harness intel. The harness is selected by the skill's invocation argument. Only the `claude-code` reference is built now. The skill produces a findings report and never edits adapter files.

**Tech Stack:** Markdown (Claude Code skill format: YAML frontmatter + body). Doc fetching at audit time uses Context7 (`/websites/code_claude`, `/anthropics/claude-code`), GitHub (`anthropics/claude-code` `CHANGELOG.md`), and `docs.claude.com`.

**Spec:** [`docs/superpowers/specs/2026-05-28-adapter-drift-audit-skill-design.md`](../specs/2026-05-28-adapter-drift-audit-skill-design.md)

---

## File Structure

- Create: `.claude/skills/adapter-drift-audit/SKILL.md` — frontmatter, when-to-use, the 6-step procedure (0–5), report format, rules. Single responsibility: *how* to audit any harness.
- Create: `.claude/skills/adapter-drift-audit/references/claude-code.md` — Claude Code intel: thesis, doc sources, contract surface map, known-fragile spots, watermark. Single responsibility: *what* to audit for one harness.

No existing files are modified. `.claude/skills/` is already an established skill location in this repo (`.claude/skills/cleanup-build-artifacts/`), so discovery is automatic.

---

### Task 1: Write the shared procedure (`SKILL.md`)

**Files:**
- Create: `.claude/skills/adapter-drift-audit/SKILL.md`

- [ ] **Step 1: Create the directory**

Run:
```bash
mkdir -p .claude/skills/adapter-drift-audit/references
```
Expected: no output; `ls .claude/skills/adapter-drift-audit` shows `references`.

- [ ] **Step 2: Write `SKILL.md` with this exact content**

````markdown
---
name: adapter-drift-audit
description: Use when checking whether a Hector adapter still matches its coding harness's current contract — auditing adapter/harness drift, verifying hook payload shapes, plugin manifest schemas, lifecycle events, or tool names are up to date, or doing periodic adapter maintenance. Takes a harness name (claude-code, pi, opencode, reasonix) as argument.
---

# Adapter Drift Audit

Audit a Hector adapter against its coding harness's **current** contract and report drift.

**Read-only.** You produce findings and recommendations. You do NOT edit adapter files, you do NOT write the watermark, and you do NOT audit `hector` core. The maintainer reads the report and decides what to change.

## When to use

- "Is the claude-code adapter still up to date with Claude Code's hooks?"
- Periodic adapter maintenance / contract-drift sweeps.
- After a harness ships a new version and you want to know what the adapter missed.

## Inputs

A harness name as the invocation argument: `claude-code`, `pi`, `opencode`, or `reasonix`. Each maps to `references/<harness>.md`. Only harnesses with a reference file can be audited.

## Procedure

### 0. Resolve target

Read the invocation argument as the harness name. Load `references/<harness>.md`. If no argument was given, list the harnesses that have a reference file under `references/` and stop — ask which one.

### 1. Read the watermark

The reference's **Watermark** section gives the baseline version / changelog date the adapter was last verified against. Note it; it scopes the changelog read in step 2.

### 2. Fetch current truth

For each entry in the reference's **Doc sources**, in this order:

1. **Context7 first** (repo + global convention). The reference pins exact library IDs, so skip `resolve-library-id` — call `query-docs` directly on each pinned ID for the contracts under audit.
2. **GitHub `CHANGELOG.md` since the watermark** — the primary signal for *what changed*. Fetch the changelog and read entries newer than the watermark.
3. **Web docs** — fallback / cross-check for any contract Context7 didn't cover.

If a source is unreachable, note it; the affected contracts become ❓ unverifiable in the report rather than silently ✅.

### 3. Compare each contract

For every row in the reference's **Contract surface map**: read the cited adapter `file:line`, compare it to the fetched truth, and classify:

- ✅ **in-sync** — adapter matches the current contract.
- ⚠️ **drift** — the contract changed; the adapter is stale.
- ❓ **unverifiable** — couldn't fetch authoritative truth this run (say which source failed).
- ✨ **new-capability-not-adopted** — the harness now offers something relevant the adapter doesn't use. A best-practice gap, not a bug.

Re-read the adapter file rather than trusting the line number — anchors drift as the adapter changes.

### 4. Emit the report

Use the **Report format** below exactly.

### 5. Propose a watermark bump

Print the suggested new `Last verified` line under a **Proposed watermark** heading. Do NOT write it into the reference file — the maintainer updates it when they act on the report. This keeps the audit read-only.

## Report format

```
# Adapter drift audit — <harness> (<date>)
Baseline: <watermark>   Current: <version / changelog date observed>

## Drift (⚠️)
- [contract #N: <name>] <adapter file:line>
  now: <current-truth summary>   (source: <doc link / Context7 id>)
  was: <what the adapter assumes>
  recommend: <concrete change>

## New capabilities not adopted (✨)
- <name> — <what it enables> (source: …) — adapter still correct.

## Unverifiable (❓)
- [contract #N] <which source was unreachable>

## In sync (✅)
- <contract #N>, <contract #M>, …

## Proposed watermark
Last verified: <date> against <harness> <version> (changelog entry: <ref>)
```

## Rules

- **Read-only**: never edit adapter files or the watermark; only report.
- **Context7 first** for schema shape; **GitHub `CHANGELOG.md`** is canonical for *what changed and when*.
- **Impact over difference**: use the reference's Thesis to judge whether a drift actually breaks gating, or is cosmetic.
- **No silent ✅**: a contract you couldn't verify is ❓, not ✅.
````

- [ ] **Step 3: Verify the frontmatter parses and the file is well-formed**

Run:
```bash
head -4 .claude/skills/adapter-drift-audit/SKILL.md
```
Expected: lines `---`, `name: adapter-drift-audit`, a `description:` line, and the surrounding `---` — i.e. valid YAML frontmatter delimited by `---`.

- [ ] **Step 4: Commit**

```bash
git add .claude/skills/adapter-drift-audit/SKILL.md
git commit -m "feat(skills): add adapter-drift-audit shared procedure"
```

---

### Task 2: Write the Claude Code reference (`references/claude-code.md`)

**Files:**
- Create: `.claude/skills/adapter-drift-audit/references/claude-code.md`

- [ ] **Step 1: Write `references/claude-code.md` with this exact content**

````markdown
# Claude Code — harness intel

Reference for `adapter-drift-audit claude-code`. Audits `adapters/claude-code/` against Claude Code's current contract.

## Thesis

Claude Code is Anthropic's agentic terminal coding tool. Its integration surface, as the adapter uses it:

- **Lifecycle hooks** fire on named events; a hook is a shell command registered in `hooks.json`. The adapter wires `PostToolUse` (gate each edit), `Stop` (session check), and `SessionStart` (clear stale state).
- **Hooks communicate by exit code + structured JSON stdout.** Exit `2` = block (stderr is fed back to the agent); exit `0` = allow. A hook can also print a `hookSpecificOutput` JSON object to inject `additionalContext` into the next turn — this is how subagent mode defers semantic evaluation.
- **Plugins** bundle hooks, skills, agents, slash-commands, and MCP servers behind a `.claude-plugin/plugin.json` manifest. `CLAUDE_PLUGIN_ROOT` is injected at hook-fire time so commands resolve regardless of install path.
- **Two model paths**: direct-API (`llm.provider` is an API-keyed provider; the hook calls the LLM itself) vs. subscription subagent (`provider: claude-code-subagent`; the hook emits a deferred payload and a subagent evaluates it next turn, billing under the parent subscription).

Use this to judge a drift's *impact*: a renamed hook-payload field silently breaks gating (high impact); a new optional manifest key the adapter ignores is cosmetic.

## Doc sources

| Source | Use for | Pointer |
|---|---|---|
| Context7 (primary) | Reference schemas: hooks, plugins, skills, sub-agents, settings | `/websites/code_claude` |
| Context7 (versioned) | Release notes pinned to a CLI version | `/anthropics/claude-code` (e.g. `v2.1.89`) |
| GitHub (drift signal) | `CHANGELOG.md` since the watermark; published plugin examples | `anthropics/claude-code` |
| Web (fallback) | Cross-check any contract Context7 missed | `docs.claude.com/en/docs/claude-code/{hooks,plugins,skills,sub-agents,settings,slash-commands}` |

Authority: for *schema shape*, prefer `/websites/code_claude` and the web hooks/plugins reference. For *what changed and when*, the GitHub `CHANGELOG.md` is canonical.

## Contract surface map

Grounded in `adapters/claude-code/` as of 2026-05-28. Line numbers are anchors — re-read the file; they speed location but may shift.

| # | Harness contract | Adapter consumer | Verify against |
|---|---|---|---|
| 1 | Hook event names `PostToolUse` / `SessionStart` / `Stop` | `adapters/claude-code/hooks/hooks.json` | hooks reference |
| 2 | Matcher syntax + file-mutating tool set (`Edit\|Write`) | `adapters/claude-code/hooks/hooks.json:5` | hooks + tool reference (watch for new mutating tools, e.g. `MultiEdit` / `NotebookEdit`) |
| 3 | `CLAUDE_PLUGIN_ROOT` injection at hook-fire time | `adapters/claude-code/hooks/hooks.json:9,19,29` | hooks reference |
| 4 | **Hook stdin payload** — `tool_input.file_path` / `.path` / `.old_string` / `.new_string` / `.content` | `adapters/claude-code/hooks/hook.sh:131,154-155` | hooks reference — **most drift-prone**; field renames silently break gating |
| 5 | Hook decision contract: exit `2` = block, `0` = allow | `adapters/claude-code/hooks/hook.sh` (all case arms) | hooks reference |
| 6 | Output envelope `hookSpecificOutput.{hookEventName,additionalContext}` | `adapters/claude-code/hooks/hook.sh:68-73,190-195` | hooks reference |
| 7 | Plugin manifest schema + location `.claude-plugin/plugin.json` | `adapters/claude-code/.claude-plugin/plugin.json` | plugins reference |
| 8 | Skill `SKILL.md` frontmatter + description-based activation | `adapters/claude-code/skills/*/SKILL.md` | skills reference |
| 9 | Subagent frontmatter (`name`/`description`/`model`/`tools`/`color`) | `adapters/claude-code/agents/hector-evaluator.md:1-7` | sub-agents reference |
| 10 | Per-dispatch subagent model override (does inline override exist yet?) | `adapters/claude-code/README.md:55-65` (flagged unresolved) | sub-agents reference |

## Known-fragile spots

Scrutinize these every run — most likely to have moved:

- **Hook stdin field names (#4).** `hook.sh` reads `.tool_input.file_path // .tool_input.path` and `.old_string // .new_string // .content`. A rename or restructure of the `PostToolUse` event payload breaks file extraction with no error — the hook exits 0 and gates nothing.
- **File-mutating tool set (#2).** The matcher is `Edit|Write`. A new mutating tool means edits via it bypass the gate entirely.
- **Output envelope (#6).** Subagent mode depends on `hookSpecificOutput.additionalContext` reaching the next turn. If the key or injection semantics change, deferred semantic evaluation silently stops.
- **Per-dispatch model override (#10).** README:55-65 documents `evaluator_model` as *advisory only* because Claude Code's subagent dispatch did not accept a per-call model override at write time. If that capability ships, flag it ✨ — the adapter can stop treating `evaluator_model` as a hint.

## Watermark

Last verified: 2026-05-28 against Claude Code v2.1.89 (initial baseline — not yet audited)
````

- [ ] **Step 2: Verify every contract-map row points at a file that exists**

Run:
```bash
cd /Users/chrisarter/Documents/projects/hector && for f in \
  adapters/claude-code/hooks/hooks.json \
  adapters/claude-code/hooks/hook.sh \
  adapters/claude-code/.claude-plugin/plugin.json \
  adapters/claude-code/agents/hector-evaluator.md \
  adapters/claude-code/README.md; do \
  test -f "$f" && echo "OK  $f" || echo "MISSING  $f"; done
```
Expected: five `OK` lines, no `MISSING`.

- [ ] **Step 3: Commit**

```bash
git add .claude/skills/adapter-drift-audit/references/claude-code.md
git commit -m "feat(skills): add claude-code reference for adapter-drift-audit"
```

---

### Task 3: Validate the skill end-to-end

No unit-test framework exists for skills; validate by exercising it.

- [ ] **Step 1: Confirm the cited line anchors still match their contracts**

Run:
```bash
cd /Users/chrisarter/Documents/projects/hector && \
  sed -n '5p' adapters/claude-code/hooks/hooks.json && \
  sed -n '131p;154,155p' adapters/claude-code/hooks/hook.sh
```
Expected: line 5 of `hooks.json` is the `"matcher": "Edit|Write"` line; line 131 of `hook.sh` extracts `FILE` via `jq` from `.tool_input.file_path // .tool_input.path`; lines 154–155 extract `OLD`/`NEW` from `.old_string` / `.new_string` / `.content`. If an anchor is wrong, fix the line number in `references/claude-code.md` (the map row), then re-commit that file.

- [ ] **Step 2: Confirm the pinned doc sources are reachable**

Use the Context7 tool to query the primary pinned ID for a hooks-reference fact, then fetch the GitHub changelog header:
- `query-docs` on `/websites/code_claude` with a query like "PostToolUse hook input JSON schema tool_input". Expect hook-reference content to return.
- `WebFetch` `https://raw.githubusercontent.com/anthropics/claude-code/main/CHANGELOG.md` (fall back to the repo's `Releases` page if the path 404s). Expect a changelog with version headers.

Expected: both sources return usable content. If `/websites/code_claude` is gone or the changelog path moved, update the **Doc sources** table in `references/claude-code.md` and re-commit.

- [ ] **Step 3: Dry-run the audit**

Invoke the skill: `/adapter-drift-audit claude-code` (or load `SKILL.md` and follow the procedure manually). Walk all 10 contract-map rows. Expected: every row resolves to a real `file:line` and lands as ✅ / ⚠️ / ❓ / ✨ with cited evidence. The run ends with a **Proposed watermark** line and writes nothing. A row that cannot find its anchor is a map bug — fix it in the reference and re-commit.

- [ ] **Step 4 (optional confidence check — do NOT commit): synthetic drift**

In a scratch copy only, rename a watched field and confirm the audit flags it:
```bash
cp adapters/claude-code/hooks/hook.sh /tmp/hook.sh.bak
sed -i '' 's/\.tool_input\.file_path/.tool_input.path_RENAMED/' adapters/claude-code/hooks/hook.sh
```
Re-run the audit; expect contract #4 to surface as ⚠️ drift. Then revert:
```bash
cp /tmp/hook.sh.bak adapters/claude-code/hooks/hook.sh && rm /tmp/hook.sh.bak
git status --short adapters/claude-code/hooks/hook.sh
```
Expected after revert: `git status` shows no change to `hook.sh`.

- [ ] **Step 5: Final state check**

Run:
```bash
cd /Users/chrisarter/Documents/projects/hector && git status --short .claude/skills/adapter-drift-audit && git log --oneline -3
```
Expected: no uncommitted changes under the skill dir; the last commits are the two `feat(skills): …` commits (plus any anchor/source fixes from Task 3).

---

## Self-Review

**Spec coverage:** §3 layout → Task 1 Step 1 + Tasks 1–2 files. §4 SKILL.md procedure + report format → Task 1 Step 2 (verbatim). §5 claude-code.md (thesis, doc sources, contract map, fragile spots, watermark) → Task 2 Step 1 (verbatim). §6 out-of-scope (read-only, no core audit, no watermark write) → encoded in SKILL.md "Read-only" rule and procedure step 5. §7 validation (dry-run, synthetic-drift, source reachability) → Task 3 steps 1–4. §8 watermark placeholder → carried into the reference's Watermark section. No gaps.

**Placeholder scan:** No TBD/TODO. The "initial baseline — not yet audited" watermark is intentional per spec §5.5/§8, replaced on first live run. All code/command steps show exact content.

**Type consistency:** Frontmatter `name: adapter-drift-audit` matches the directory `.claude/skills/adapter-drift-audit/` and the spec throughout. Classification glyphs (✅/⚠️/❓/✨) are identical in SKILL.md procedure step 3, its report format, and the reference's fragile-spots usage. Doc-source IDs (`/websites/code_claude`, `/anthropics/claude-code`) match the spec §5.2. Contract-map row count (10) and contents match spec §5.3.
