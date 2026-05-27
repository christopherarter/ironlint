# Hector — Reasonix adapter (PreToolUse pivot)

**Status:** Option A landed; adapter scaffold updated to PreToolUse on 2026-05-27. Core CLI now ships `--content` (see §5A). Per-tool gaps remain — `multi_edit` is still a no-op; `bash`-shell-out writes still bypass the hook.
**Date:** 2026-05-25
**Owner:** dynamik-dev
**Companion to:** [`overview.md`](./overview.md), [`docs/adapters/claude-code.md`](../docs/adapters/claude-code.md), [`docs/adapters/opencode.md`](../docs/adapters/opencode.md)
**Scaffold:** [`adapters/reasonix/`](../adapters/reasonix/) — PreToolUse hook handling `write_file` and `edit_file`.

---

## 1. Summary

A PostToolUse-shaped Reasonix adapter does not serve hector's purpose. Reasonix's `PostToolUse` is documented as "non-gating, warning-only on errors" — exit code 2 surfaces the verdict to the agent's next turn but does not reject the tool call, so the bad edit lands on disk regardless of the verdict. Hector's stated value is physical prevention; only `PreToolUse` ("can gate execution by returning exit code 2") delivers that.

Pivoting to `PreToolUse` is the right call. It is also more work than swapping a hook name. The core CLI has no input mode for "evaluate this proposed file content without reading the on-disk version" — `--file` reads from disk, and `--diff` *also* reads from disk (see §4). Without that, a `PreToolUse` adapter has nothing useful to ask hector. This spec documents the gap and ranks options for closing it.

## 2. Reasonix protocol (verified)

Settings file (two scopes; project takes precedence):
- Global: `~/.reasonix/settings.json`
- Project: `<project>/.reasonix/settings.json`

Hook entry shape:

```json
{
  "command": "/abs/path/to/hook.sh",
  "match": "^(write_file|edit_file|multi_edit)$",
  "description": "...",
  "timeout": 30000
}
```

Stdin payload to the hook:

```json
{
  "event": "PreToolUse",
  "cwd": "/workspace",
  "toolName": "edit_file",
  "toolArgs": { "path": "src/foo.ts", "search": "...", "replace": "..." },
  "turn": 3
}
```

Tool schemas relevant to hector (from `src/tools/filesystem.ts` on `main`):
- `write_file` — `{ path, content }`
- `edit_file` — `{ path, search, replace }` (whitespace-sensitive plain text; `search` must be unique)
- `multi_edit` — array of per-file `{ path, edits: [{ search, replace }, ...] }`

Docs vs. source mismatch worth knowing: the configuration page example uses `^(write|edit_file|bash)$`, but the actual tool name is `write_file`, not `write`. Match against the source names.

Lifecycle events and gating:
- `PreToolUse` — runs **before** the tool. Exit 2 gates execution.
- `PostToolUse` — runs **after**. Exit 2 is warning-only. Non-gating.
- `UserPromptSubmit` — runs before user input. Exit 2 blocks the message.
- `Stop` — runs on `/quit` or session exit. Non-gating. (Reasonix has no `SessionStart` equivalent.)

No `${CLAUDE_PLUGIN_ROOT}`-equivalent env var is documented. Absolute paths in the `command` field are required.

## 3. Why PostToolUse fails the mission

The Claude Code adapter gets away with PostToolUse-style gating because Claude Code's `PostToolUse` *does* block. Reasonix made the opposite choice: their `PostToolUse` is purely advisory. From the docs:

> **PostToolUse** – Executes after a tool completes; non-gating, warning-only on errors

A hector verdict emitted on PostToolUse in Reasonix reaches the agent's next turn via stderr → context, and the agent can self-correct. That's better than nothing, but it has none of the guarantees hector is built to make: the bad code is on disk, downstream tooling (build, dev server, file watchers, other agents tailing the workspace) sees it, and any rule whose purpose is "this MUST NOT enter the working tree" has been bypassed by definition.

Don't ship PostToolUse as the recommended Reasonix path. It misframes what hector is.

## 4. The core gap blocking PreToolUse

`hector check` has two input modes today (`crates/hector-cli/src/cli.rs:18–48`, `crates/hector-core/src/runner.rs:809–825`):

| Mode | Source of post-edit content | Source of diff |
|---|---|---|
| `--file <path>` | reads `<path>` from disk | empty |
| `--diff <path-to-diff-file>` | reads `<file>` from disk | reads the diff file |

The runner comment on `--diff` is explicit:

> Read the post-edit file from disk so AST rules, disable directives, and any other content-based engine see real content. In the agent flow, diff mode runs *after* the agent's edit has landed on disk, so reading the file here is the correct semantics (P0-5, P0-7). A missing file falls back to empty content — AST will then no-op rather than crashing the runner.

Both modes assume the edit has landed. PreToolUse, by definition, runs before the edit lands. If the adapter calls `hector check --file <path>` in a PreToolUse hook, it gates against the *current* file content — what was already on disk before the agent touched it — which is meaningless: that content presumably already passed checks, otherwise we wouldn't be editing it.

This is the same architectural limit that prevents an OpenCode `tool.execute.before` adapter — called out in [`docs/adapters/opencode.md`](../docs/adapters/opencode.md):

> No `tool.execute.before` interception. Hector engines read files from disk; running `before` would require synthesising the post-edit content in memory.

Reasonix forces the issue.

## 5. Options for closing the gap

Ranked. Recommend **A**.

### A. New CLI input mode: `--file <path> --content -` (stdin)

Add a CLI flag that lets the adapter feed proposed post-edit content via stdin while keeping the path for scope matching, relativization, and AST language detection.

```
hector check --file src/foo.ts --content - --config .hector.yml
< <proposed-content-bytes>
```

Runner changes: third `CheckInput` variant `Proposed { path: PathBuf, content: String, diff: String }`. `check_inner` already takes `(path, content, diff)` post-normalization (`runner.rs:812`) — the new variant just skips the `fs::read_to_string` and accepts the content directly. Scope matchers, `relativize`, disable directives, AST, script, semantic — none care where the content came from. The only thing that changes is the source.

Adapter changes: small. The hook synthesizes the post-edit content (trivially for `write_file` since `toolArgs.content` *is* the new content; for `edit_file` apply the search/replace against the on-disk file, fail closed if `search` doesn't match unique), pipes it into hector with `--content -`, exits with hector's exit code.

Why this is the right shape:
- Path stays real, so policy-file scope (`scope: "src/**/*.ts"`, skip globs, baseline matching) keeps working exactly as authored.
- One new variant, additive. No verdict-schema change. No telemetry-schema change.
- Symmetric with how the OpenCode adapter would want it too — pays off twice.

**Implementation note (2026-05-27).** Landed Option A as a CLI-only change — no new `CheckInput` variant. `CheckInput::File { path, content }` already accepts the content as a `String` from the caller (the disk read lives in `commands/check.rs`, not in the runner), so `--content` simply short-circuits that read. Two adjacent runner fixes were needed to make the spec's "none care where it came from" claim actually true:

1. **Authoritative empty content** — `evaluate_one_rule` was collapsing empty content onto `None` for the AST engine, which surfaced as an `__internal` violation. Empty *proposed* content (a `write_file` creating an empty file) is now distinguished from empty *disk-read* content (a read failure) via a `content_authoritative` track in `check_inner`.
2. **`relativize` through the parent** — non-existent paths (the `write_file` case) used to fall back to the literal `PathBuf`, which on macOS's `/var → /private/var` symlink layout meant the scope-matching `strip_prefix` failed and rules were silently out of scope. `canonicalize_through_parent` walks up to the deepest existing ancestor and rebuilds a canonical path.

The script engine's `{file}` / `HECTOR_FILE` substitution still points at the on-disk path — script rules under `--content` read pre-edit content. Documented as a known limitation in `hector check --help` and in [`adapters/reasonix/README.md`](../adapters/reasonix/README.md).

### B. Adapter writes a tempfile, calls `--file`

The adapter materializes the proposed content to a tempfile and runs `hector check --file <tempfile>`. No core change.

Why this is the wrong shape: scope rules and `match_path` use `relativize(&path, &self.config_dir)` (`runner.rs:867`). A tempfile at `/tmp/hector-XXXX.ts` doesn't relativize to anything meaningful inside the project's config dir, so any rule with a path-scoped filter — which is most of them — silently fails to match. You'd "pass" not because the code is clean but because no rule applied. Strictly worse than no adapter.

You could mirror the project layout inside a temp directory rooted at a fake config_dir, but at that point you're rebuilding the project tree on every keystroke for no semantic gain over option A.

### C. Block in `Stop` only, treat `PreToolUse` as a no-op

Skip per-edit gating entirely; let edits land, run `hector check --session` on `Stop` (which Reasonix gates? — verify, but docs say non-gating, so this also fails the mission). Mentioned for completeness, do not pursue.

## 6. Adapter sketch (after option A lands)

```bash
#!/usr/bin/env bash
# adapters/reasonix/hooks/hook.sh pre-tool-use
set -euo pipefail
EVENT=$(cat)
PROJECT_ROOT=$(jq -r '.cwd // empty' <<<"$EVENT")
CONFIG="$PROJECT_ROOT/.hector.yml"
[[ -f "$CONFIG" ]] || exit 0

TOOL=$(jq -r '.toolName' <<<"$EVENT")
REL=$(jq -r '.toolArgs.path' <<<"$EVENT")
[[ -n "$REL" ]] || exit 0
FILE="$REL"; [[ "$FILE" = /* ]] || FILE="$PROJECT_ROOT/$FILE"

case "$TOOL" in
  write_file)
    PROPOSED=$(jq -r '.toolArgs.content' <<<"$EVENT")
    ;;
  edit_file)
    SEARCH=$(jq -r '.toolArgs.search' <<<"$EVENT")
    REPLACE=$(jq -r '.toolArgs.replace' <<<"$EVENT")
    PROPOSED=$(apply_unique_edit "$FILE" "$SEARCH" "$REPLACE") || {
      echo "hector: refusing edit — could not synthesize post-edit content" >&2
      exit 2
    }
    ;;
  multi_edit)
    # Fold the per-file edits array into N (path, content) pairs and run
    # `hector check` per file. First block wins; exit 2.
    ...
    ;;
  *) exit 0 ;;
esac

printf '%s' "$PROPOSED" | hector check --file "$FILE" --content - --config "$CONFIG" --format json
```

`apply_unique_edit` is the same uniqueness check Reasonix does internally (read file, ensure `search` appears exactly once, substitute, return result). Failing closed there matches Reasonix's own refusal semantics for ambiguous edits.

Notes:
- `multi_edit` is the awkward one — N files in one tool call, one verdict per file, first block wins. The first violation should also block the entire tool call (Reasonix runs the whole `multi_edit` atomically or not at all), so exit 2 after any failing file.
- Short-circuit edits to `.hector.yml` / `.bully.yml` themselves — same reason as the Claude Code adapter (trust fingerprint mid-edit produces misleading errors).
- No session recording. Session-engine rules need a separate `Stop` hook; non-gating in Reasonix, but useful as a post-session audit.

## 7. What the protocol cannot do

A few things to be honest about so the docs don't oversell:

- **No precondition for `read_file`.** Hector cannot demand a file was read before edit in Reasonix the way Claude Code's adapter can hint to the model — Reasonix already enforces this itself in `edit_file` (refuses if not read this session). We get this for free; no hector code path to write.
- **`apply_patch`-equivalent gating.** Reasonix has no multi-file patch tool today (per the source). If one is added later, plan an adapter update like OpenCode's `apply_patch` caveat.
- **Shell-injected file writes via `bash`.** A `bash` tool call with `cat > foo.ts` bypasses the PreToolUse hook entirely (it matches `bash`, not `write_file`). Two responses: (1) match `bash` in the same hook and reject any command that looks like a file write — fragile; (2) explicitly document the limit. Recommend (2).

## 8. Migration from the PostToolUse scaffold

[`adapters/reasonix/`](../adapters/reasonix/) currently contains a working PostToolUse adapter. Most of it is throwaway under the pivot. What to keep:

| Piece | Keep? | Why |
|---|---|---|
| `hooks/hook.sh` stdin parsing (`cwd`, `toolArgs.path`, basename short-circuit) | Yes | Identical for PreToolUse; lift verbatim. |
| `hooks/hook.sh` invocation of `hector check --file` | No | Replace with `--content -` (after §5 lands). |
| `hooks/settings.example.json` `PostToolUse` block | No | Swap to `PreToolUse`. |
| `README.md` comparison table | Update | Pivot the column from "exit 2 warning-only" to "exit 2 blocks". |
| Global registration in `~/.reasonix/settings.json` (PostToolUse) | Disable | Either remove or flip to `PreToolUse` once the adapter is real. Currently a no-op on any project without `.hector.yml`, so leaving it does no harm — but it gives the wrong impression of what hector does, so prefer to disable until the pivot ships. |

Don't delete the PostToolUse scaffold until the PreToolUse adapter is at least a draft — the path parsing and config detection logic is reusable.

## 9. Open questions

1. **Does `apply_unique_edit` belong in core or in the adapter?** Every adapter that hits this problem (Reasonix `edit_file`, a future Aider adapter, OpenCode's `before`) needs the same primitive: "given (path, search, replace), produce the post-edit content or refuse." Worth a tiny core helper exposed as `hector synthesize-edit --file <path> --search <s> --replace <r>` that writes the result to stdout. Lower priority than option A in §5; can be deferred.

2. **Does PreToolUse need session recording?** Probably not — session rules evaluate the whole changeset and are meant to run on `Stop`. PreToolUse should be per-edit only. Wire `Stop` separately if anyone asks for it.

3. **`multi_edit` atomicity vs. partial verdicts.** Reasonix's `multi_edit` is atomic. If hector blocks file 3 of 5, do we want a verdict that names file 3 only, or all five with three "would-have-passed" entries? Default to "name the first blocker, exit"; revisit if users want fuller reports.

4. **Direct-API vs. subagent mode.** Reasonix is its own LLM; there's no subscription-vs-API split to manage. Default to direct-API only. Skip the subagent-payload path entirely.

5. **Telemetry.** PreToolUse fires on every proposed edit; some get rejected and never become real edits. Tag verdicts with `event: "PreToolUse"` so dashboards can separate "edits the agent attempted" from "edits that landed."

## 10. Recommended next steps

1. Land option A in core: new `CheckInput` variant + `--content -` CLI flag. Small, additive, useful beyond Reasonix.
2. Rewrite `adapters/reasonix/hooks/hook.sh` against the new flag. Drop the PostToolUse case statement.
3. Update [`adapters/reasonix/README.md`](../adapters/reasonix/README.md) to describe PreToolUse semantics and the `bash`-shell-out gap from §7.
4. Move docs from `adapters/reasonix/README.md` into a proper `docs/adapters/reasonix.md` once the adapter is functional (matches the layout of claude-code and opencode adapters).
5. Disable or rewrite the entry in `~/.reasonix/settings.json` so it stops advertising PostToolUse as the install path.
