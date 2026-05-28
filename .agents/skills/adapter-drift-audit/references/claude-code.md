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
