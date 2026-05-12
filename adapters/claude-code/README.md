# Hector — Claude Code adapter

`/plugin install` integration for Claude Code. Provides:

- `PostToolUse` hook: runs `hector check` on every `Edit` or `Write` tool call.
- `Stop` hook: runs `hector check --session` over the accumulated changeset.
- `SessionStart` hook: clears stale session state from a previous run.
- Skills: `/hector-init`, `/hector-author`, `/hector-review`.

## Install

1. Install the `hector` binary (`cargo install hector`, or use a release binary).
2. Add this plugin via your Claude Code plugin manager.
3. Run `/hector-init` in a project to scaffold `.hector.yml`.
4. Review the config and run `hector trust` to fingerprint it.
5. Edit any file — the PostToolUse hook will gate edits against the rules.

## Requirements

- `hector` binary on PATH.
- `jq` on PATH (parse PostToolUse event payloads).
- `bash` (the hook script is bash).
