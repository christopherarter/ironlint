# IronLint — Claude Code adapter

`PostToolUse` hook integration for Claude Code. Runs `ironlint check` on every
`Edit` or `Write` tool call, checking the edit against your project's
`.ironlint.yml` policy.

> **Timing:** this adapter fires on **PostToolUse**, so the edit has *already*
> been written to disk by the time the hook runs. A block (exit 2) does not
> revert the write — it surfaces the check's message to the agent as feedback,
> and the agent is expected to correct it on its next turn. Pre-write blocking
> (PreToolUse) is planned; the reasonix and pi adapters already gate pre-write.

> **Note:** this adapter installs via a direct settings patch (see below). The
> `.claude-plugin/` plugin-packaging layout under this directory is kept for
> users who prefer the marketplace plugin workflow, but `ironlint init` is the
> recommended path.

## Install

```bash
ironlint init --harness claude-code
```

This auto-detects Claude Code and patches `<project>/.claude/settings.json`
(or `~/.claude/settings.json` with `--global`) to register a `PostToolUse`
hook matching `Edit|Write`. The adapter artifacts are written atomically to
`~/.config/ironlint/adapters/claude-code/` and a `.ironlint-adapter.json` sidecar
(per-file sha256 + version) is placed alongside them. A backup of the prior
settings file is saved as `<settings>.bak` on the first write; re-runs are
idempotent (unchanged → "already present", changed artifact → "updated").

Verify the install:

```bash
ironlint doctor
```

To remove the hook:

```bash
ironlint init --uninstall --harness claude-code
```

This removes the hook entry, the materialized artifact, and the sidecar from
`~/.config/ironlint/adapters/claude-code/`. Your `.ironlint.yml` and trust store
are untouched.

## Manual fallback

Use these steps if the `ironlint` binary is not available:

1. Install the `ironlint` binary (`cargo install --git https://github.com/christopherarter/ironlint ironlint-cli`, or use a release binary).
2. Add this plugin via your Claude Code plugin manager.
3. Run `ironlint init` in a project to scaffold `.ironlint.yml`.
4. Review the config and run `ironlint trust` to fingerprint it.
5. Edit any file — the PostToolUse hook will check edits against the policy.

## Requirements

- `ironlint` binary on PATH.
- `bash`, `jq`, `awk` on PATH (required at hook runtime).

## How the hooks resolve

`hooks/hooks.json` dispatches the PostToolUse event to `"${CLAUDE_PLUGIN_ROOT}/hooks/hook.sh"`.
`CLAUDE_PLUGIN_ROOT` is set by Claude Code at hook-fire time and points to the
plugin's installed directory (wherever the plugin manager unpacked this adapter).
You do **not** set it yourself.

If a hook fails with `hook.sh: No such file or directory`, the plugin is not
installed where Claude Code expects. Reinstall via `ironlint init --harness claude-code`
or, for local development, symlink this directory under your plugins root. See
[`docs/adapters/claude-code.md`](../../docs/adapters/claude-code.md) for full
install paths and diagnostic steps.
