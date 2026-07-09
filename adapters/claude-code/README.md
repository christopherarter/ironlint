# IronLint — Claude Code adapter

`PreToolUse` hook integration for Claude Code. Runs `ironlint check` on every
`Edit`, `Write`, `MultiEdit`, or `NotebookEdit` tool call **before** the edit
lands on disk, checking the proposed content against your project's
`.ironlint.yml` policy.

> **Timing:** this adapter fires on **PreToolUse**, so ironlint sees the
> proposed content — `tool_input.content` for `Write`, or `old_string` ->
> `new_string` applied to the current on-disk file for `Edit` — before Claude
> Code writes it. A block (exit 2) **prevents the write**: Claude Code never
> touches disk, and the check's message is surfaced to the model on stderr as
> the denial reason so it can correct course on its next turn. This mirrors
> the codex and pi adapters, which also gate pre-write.

> **Note:** this adapter installs via a direct settings patch (see below). The
> `.claude-plugin/` plugin-packaging layout under this directory is kept for
> users who prefer the marketplace plugin workflow, but `ironlint init` is the
> recommended path.

## Install

```bash
ironlint init --harness claude-code
```

This auto-detects Claude Code and patches `<project>/.claude/settings.local.json`
(or `~/.claude/settings.json` with `--global`) to register a `PreToolUse`
hook matching `Edit|Write|MultiEdit|NotebookEdit`. A local (project-scope)
install always targets `settings.local.json` — the personal, gitignored
settings file Claude Code merges in — never the committable `settings.json`,
so the machine-specific absolute hook path never lands in version control.
The adapter artifacts are written atomically to
`~/.config/ironlint/adapters/claude-code/` and a `.ironlint-adapter.json` sidecar
(per-file sha256) is placed alongside them. A backup of the prior
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

## Bash gate

In addition to file edits, this adapter gates `Bash` (the agent's shell tool —
`tool_name:"Bash"`, command in `tool_input.command`). Commands that would let
the agent free itself — `ironlint trust`, or a Bash write to `.ironlint.yml` /
`.ironlint/scripts/` — are denied (hook exit 2, reason on stderr). Ordinary
commands are not slowed: a substring pre-filter skips the decision entirely
for commands that never mention `ironlint` or `.ironlint`. The deny decision
is shared across every adapter via `ironlint gate-bash`. The branch runs
before the config-existence check, so it fires even in a project with no
`.ironlint.yml` — exactly when the agent is most motivated to self-trust. See
`docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`
for the threat model and the documented known gap (variable-substitution
indirection).

## Manual fallback

Use these steps if the `ironlint` binary is not available:

1. Install the `ironlint` binary (`cargo install --git https://github.com/christopherarter/ironlint ironlint-cli`, or use a release binary).
2. Add this plugin via your Claude Code plugin manager.
3. Run `ironlint init` in a project to scaffold `.ironlint.yml`.
4. Review the config and run `ironlint trust` to fingerprint it.
5. Edit any file — the PreToolUse hook checks the proposed edit against the
   policy *before* it lands; a violating edit is blocked and never written.

## Requirements

- `ironlint` binary on PATH.
- `bash`, `jq`, `python3` on PATH (required at hook runtime — `python3` applies
  `Edit`'s `old_string`/`new_string` substitution to synthesize the proposed
  content).

## How the hooks resolve

`hooks/hooks.json` dispatches the PreToolUse event to `"${CLAUDE_PLUGIN_ROOT}/hooks/hook.sh"`.
`CLAUDE_PLUGIN_ROOT` is set by Claude Code at hook-fire time and points to the
plugin's installed directory (wherever the plugin manager unpacked this adapter).
You do **not** set it yourself.

If a hook fails with `hook.sh: No such file or directory`, the plugin is not
installed where Claude Code expects. Reinstall via `ironlint init --harness claude-code`
or, for local development, symlink this directory under your plugins root. See
[`docs/adapters/claude-code.md`](../../docs/adapters/claude-code.md) for full
install paths and diagnostic steps.
