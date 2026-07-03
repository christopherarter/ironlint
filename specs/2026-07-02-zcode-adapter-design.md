# ZCode Adapter — Design

## Context

[ZCode](https://zcode.z.ai) is Z.AI's agentic terminal coding tool ("Official Harness for GLM-5.2"). Investigation against a live install (`~/.zcode/`) confirms it is a **Claude-Code plugin-system fork**: it ships the official `claude-plugins-official` marketplace, consumes Claude-Code-shaped plugins, and exposes a `Hook` plugin component with Claude-Code-compatible semantics.

This spec designs the ironlint adapter that wires ironlint's pre-write gate into ZCode.

## Thesis

ZCode is the **fifth supported harness**. It gates file edits via a `PreToolUse` hook (block-before-disk) inside a plugin directory, identical in event/exit-code contract to Claude Code but differing in **how the hook is registered**: ZCode discovers plugins from a directory tree (`.zcode-plugin/plugin.json` + `hooks/hooks.json`), not from a settings.json hook-array patch. This makes ZCode a hybrid of the two existing `HarnessKind` variants — neither `JsonHookSpec` (scripts + settings.json patch) nor `PluginSpec` (single TS file) fits, so the design introduces a third variant.

## Ground truth (from `~/.zcode/`)

| Contract | Observed value | Source |
|---|---|---|
| App home | `~/.zcode/` | filesystem |
| App prefs | `~/.zcode/v2/setting.json` (singular) | filesystem — app prefs, **not** a hooks file |
| Plugin manifest | `.zcode-plugin/plugin.json` (renamed from Claude Code's `.claude-plugin/plugin.json`) | `~/.zcode/cli/plugins/cache/zcode-plugins-official/document-skills/0.1.0/.zcode-plugin/plugin.json` |
| Hook config | `hooks/hooks.json` with wrapper format `{ "hooks": { "<Event>": [{ "matcher": "...", "hooks": [{ "type": "command", "command": "...", "timeout": N }] }] } }` | `~/.zcode/cli/plugins/cache/zcode-plugins-official/android-emulator/0.1.0/hooks/hooks.json` |
| Plugin root var | `${ZCODE_PROJECT_DIR}` (project root) and `${ZCODE_PLUGIN_DATA}` (per-plugin data dir) — the ZCode rename of `${CLAUDE_PLUGIN_ROOT}` | android-emulator `.zcode-plugin/plugin.json` `mcpServers.*.cwd`/`env` |
| Hook events | `PreToolUse`, `PostToolUse`, `UserPromptSubmit`, `SessionEnd` | ZCode Hook docs (search-cache; SPA page) |
| Matcher | tool name regex, e.g. `Write|Edit` | ZCode Hook docs |
| Gating contract | exit `0` = allow, **nonzero = block** (PreToolUse) | ZCode Hook docs (Claude-Code-compatible) |
| Stdin payload | `{ tool_name, tool_input: { file_path, content | old_string+new_string } }` (Claude-Code shape) | Claude-Code parity; confirmed by ZCode's marketplace consuming `claude-plugins-official` |
| Marketplaces | `~/.zcode/cli/plugins/known_marketplaces.json` (registry) + `~/.zcode/cli/plugins/marketplaces/` (cloned sources) | filesystem |
| Plugin install cache | `~/.zcode/cli/plugins/cache/<marketplace>/<plugin>/<version>/` | filesystem |
| Installed-plugin data | `~/.zcode/cli/plugins/data/<plugin>@<marketplace>/` | filesystem |
| Plugin DB | `~/.zcode/cli/db/db.sqlite` (`local_setting` table) | filesystem |

## Design decisions

### D1 — New `HarnessKind` variant: `PluginTree`

Neither existing variant fits:

- `JsonHookSpec` writes hook scripts to `~/.config/ironlint/adapters/<name>/` and **patches a settings.json** with a hook-array entry. ZCode has no settings.json hook array — hooks live in `hooks/hooks.json` *inside the plugin dir*.
- `PluginSpec` writes a **single** TS source file to an extensions dir. ZCode needs a **multi-file plugin tree**: `.zcode-plugin/plugin.json` + `hooks/hook.sh` + `hooks/hooks.json` + `hooks/synthesize_diff.sh` + `skills/`.

Add a third variant:

```rust
pub enum HarnessKind {
    JsonHook(JsonHookSpec),   // claude-code, reasonix
    Plugin(PluginSpec),       // pi, opencode  (single TS file)
    PluginTree(PluginTreeSpec), // zcode       (multi-file plugin dir)
}
```

`PluginTreeSpec` materializes a directory of `(relpath, bytes)` files under a plugin dir, writes a sidecar covering all of them, and (unlike `JsonHookSpec`) does **not** patch any external settings file — the `hooks/hooks.json` is one of the materialized files, so registration is implicit in the tree.

### D2 — `PreToolUse`, not `PostToolUse`

ZCode's `PreToolUse` can physically block a tool call before disk is touched (exit nonzero = block). This is strictly better than claude-code's `PostToolUse` (the edit already landed; ironlint checks after the fact). The adapter therefore mirrors the **reasonix** posture, not the claude-code one. The hook script pipes the proposed content on stdin to `ironlint check --file <path> --content -`.

### D3 — Reuse the claude-code hook script logic, adapted

ZCode's PreToolUse stdin payload is Claude-Code-shaped (`tool_name`, `tool_input.file_path`, `tool_input.content` / `old_string`+`new_string`). The claude-code `hook.sh` already parses exactly this shape via `jq`. The ZCode `hook.sh` is the claude-code script with two changes:

1. **No `synthesize_diff.sh` / `--diff` path.** PreToolUse has the proposed content directly (`tool_input.content` for Write; for Edit, apply `old_string`→`new_string` to the on-disk file). Use `ironlint check --file <path> --content -` (the reasonix model), not `--diff`. This drops `synthesize_diff.sh` entirely — simpler, and the pre-write content is exactly what we want to check.
2. **`${ZCODE_PROJECT_DIR}` fallback for project root.** ZCode injects this env var at hook-fire time; use it in preference to `pwd` so the root is correct even if the hook's cwd differs. (Claude Code's `pwd`-based resolution stays as fallback.)

### D4 — Install location: a plugin directory under the ironlint adapters dir

The existing `JsonHookSpec` writes scripts to `~/.config/ironlint/adapters/<name>/`. For ZCode, materialize the **full plugin tree** to `~/.config/ironlint/adapters/zcode/` (same parent), preserving the layout ZCode expects:

```
~/.config/ironlint/adapters/zcode/
  .zcode-plugin/plugin.json
  hooks/hook.sh            (executable)
  hooks/hooks.json
  skills/ironlint-config/SKILL.md
  .ironlint-adapter.json   (sidecar — version + per-file sha256)
```

Then **symlink** (or instruct the user to add) this dir as a local marketplace / plugin source in ZCode's plugin manager (`Settings → Plugins → add local directory`). The adapter cannot write to `~/.zcode/cli/plugins/cache/` directly — that path is managed by ZCode's marketplace clone+install flow and a sqlite registry; writing there by hand would be invisible to ZCode and overwritten on next refresh. The supported onboarding path is: materialize the plugin tree, then point ZCode at it.

> **Open question (deferred to execution):** whether ZCode supports a project-local plugin dir (e.g. `.zcode/plugins/`) like opencode/pi, which would let `ironlint init --harness zcode` wire per-project without marketplace registration. The live install had no `.zcode/` in the seeded project, only `~/.zcode/`. The adapter will support a `--global` (default) install to the ironlint adapters dir + a documented manual step to register it in ZCode; per-project plugin dir support is a follow-up if ZCode exposes one.

### D5 — Detection: `~/.zcode` dir

`is_detected` for the zcode harness returns `env.home.join(".zcode").is_dir()` — mirrors the claude-code (`~/.claude`) and pi (`~/.pi`) detection.

### D6 — Skill: shared `ironlint-config`

Like all four existing harnesses, ZCode gets the shared `ironlint-config` `SKILL.md` installed to its skills dir. ZCode's skills live under `<plugin-dir>/skills/`, so for the tree-based install the skill is part of the materialized tree (no separate skill-dir resolution). For consistency with the `SkillSpec` model used by `install_skill`/`uninstall_skill`, the zcode harness still exposes a `SkillSpec` pointing *inside* the plugin tree — but `install_skill` for a `PluginTree` harness is a no-op (the skill is already in the tree written by `install`). This keeps `ironlint init`'s per-harness skill-install loop uniform.

## Non-goals

- No MCP server, no LSP, no slash-commands, no subagents in the ironlint zcode plugin — only the hook + the authoring skill. (ZCode supports all of these; ironlint needs only the hook.)
- No writing into `~/.zcode/cli/plugins/cache/` or the sqlite DB — ZCode's marketplace owns those.
- No `PostToolUse` / `SessionEnd` / `UserPromptSubmit` hooks — only `PreToolUse` (the gate). Claude-code's `Stop`/`SessionStart` hooks exist for subagent semantic eval, which ironlint dropped (spec `2026-06-11-remove-llm-evaluation`); ZCode doesn't need them.
- No migration of the claude-code `synthesize_diff.sh` — the `--content` path is sufficient for pre-write gating.

## Risks

- **SPA docs unverifiable at write time.** The ZCode Hook docs page is a Next.js SPA; Crawl4AI didn't render it. The contract above is confirmed from the **live install** (plugin manifests, `hooks/hooks.json` shape) + search-engine cache of the docs. The drift-audit reference (Task of the plan) records the watermark as "not yet audited against a rendered docs page" so the first audit re-verifies.
- **Plugin-root variable name.** `${ZCODE_PROJECT_DIR}` is observed in `mcpServers` config, not confirmed for hook `command` strings. The hook script falls back to `pwd`, so a wrong variable name degrades to the claude-code behavior (correct cwd), not a hard failure.
- **Marketplace registration is a manual step.** Unlike the other four harnesses where `ironlint init` fully wires the hook, ZCode requires the user to add the materialized plugin dir in ZCode's plugin manager. This is a UX regression but unavoidable without reverse-engineering ZCode's sqlite install flow (out of scope; would break on ZCode updates).
