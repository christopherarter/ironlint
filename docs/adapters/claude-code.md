# Claude Code adapter

The Claude Code adapter runs your IronLint checks every time Claude edits a file. When an edit breaks a check, Claude Code rejects it on the spot, hands Claude the verdict, and Claude rewrites the change to comply. You stop having to remember to run `ironlint check` yourself; the check is always on.

The adapter ships in this repo at `adapters/claude-code/`. It predates the 0.3 gates redesign, so its alignment to the check ABI (`$IRONLINT_FILE`, the proposed post-edit content on stdin, any nonzero exit blocks) is still in progress under Plan 4; the `.ironlint.yml` check format shown below is current regardless.

## Install

With the `ironlint` binary and `jq` on your `PATH`, one command wires the hook and scaffolds a trusted config:

```bash
ironlint init --harness claude-code
```

This patches `<project>/.claude/settings.local.json` (or `~/.claude/settings.json` with `--global`) to register a `PreToolUse` hook matching `Edit|Write|MultiEdit|NotebookEdit`, and materializes the hook scripts to `~/.config/ironlint/adapters/claude-code/` with a `.ironlint-adapter.json` sidecar (per-file sha256 + version). A local (project-scope) install always targets `settings.local.json` — the personal, gitignored settings file Claude Code merges in — never the committable `settings.json`, so the machine-specific absolute hook path never lands in version control. A backup of the prior settings file is written as `<settings>.bak` on the first patch; re-runs are idempotent. Restart (or reload) Claude Code so it picks up the new hook, then verify:

```bash
ironlint doctor
```

To remove the hook, its artifacts, and the sidecar (leaving `.ironlint.yml` and the trust store):

```bash
ironlint init --uninstall --harness claude-code
```

This settings-hook install gives you the **check** and installs the `ironlint-config` authoring skill into `.claude/skills/ironlint-config/`. `/ironlint-init` and `/ironlint-review` ship with the plugin package — see [Author and review checks from inside Claude](#author-and-review-checks-from-inside-claude) below.

If you wrote `.ironlint.yml` by hand instead of letting `ironlint init` scaffold it, trust it before checks will run:

```bash
ironlint trust
```

IronLint runs the commands in your config, so it refuses to run one it hasn't seen. `ironlint trust` records the config in the trust store; any later edit invalidates it and you re-sign. See [The trust store](../security/trust.md) for why.

## Watch it block an edit

Here is the whole point of the adapter, end to end. Suppose your `.ironlint.yml` bans `DEBUG` markers in TypeScript:

```yaml
# .ironlint.yml
checks:
  no-debug:
    files: "**/*.ts"
    run: "! grep -n 'DEBUG'"  # proposed content arrives on stdin
```

Ask Claude to add a `DEBUG` marker to a `.ts` file. The instant Claude writes the edit, the adapter runs `ironlint check` against that file, the `no-debug` check exits nonzero, and Claude Code rejects the edit. Claude reads the returned block message — the check's own output — sees that it broke `no-debug`, and rewrites the change without the marker. The retry happens in the transcript while you watch; you never touched the keyboard.

A clean edit, one that breaks no check, lands normally and you see nothing at all. That silence is the adapter working.

## What runs, and when

Every adapter follows the [same lifecycle](README.md#what-adapters-do); here is how Claude Code wires it:

**Before every edit.** When Claude is about to run `Write`, `Edit`, `MultiEdit`, or `NotebookEdit`, the adapter synthesizes the proposed post-edit content and runs `ironlint check --file <path>` against it. A block rejects the edit and Claude retries. This is the check you saw above.

- **Write** — the tool already carries the full post-edit content; ironlint gates it directly.
- **Edit** — the adapter applies `old_string → new_string` to the current on-disk file and gates the result.
- **MultiEdit** — the adapter folds the `edits` array onto the on-disk file **in order**, each edit against the result of the previous ones, mirroring Claude Code's own apply and uniqueness rules, then gates the final content. If an edit can't apply cleanly (a non-`replace_all` `old_string` that isn't unique after the prior edits), the adapter blocks — exactly as Claude Code would refuse it.
- **NotebookEdit** — the adapter gates the edited cell's proposed `new_source` (piped on stdin) with `$IRONLINT_FILE` set to the `.ipynb` path. This runs the cell source against any check whose `files` glob matches the **notebook path**, so a check scoped to `*.py` will **not** match a `.ipynb` — scope a check to `*.ipynb` to gate notebook cell edits. A `delete` edit removes a cell and has no proposed content, so it is not gated (allowed through).

## Timeout budget

This hook sets no ironlint-specific timeout of its own, so it isn't affected by [IronLint's default per-check cap](README.md#timeout-budget). If you hand-add a `timeout` to this hook's entry in `.claude/settings.local.json` (or `~/.claude/settings.json` for a global install), keep it at or above your worst-case sequential-check budget (`K × execution.timeout_secs` for `K` checks matching a file) — the same rule that applies to any JSON-hook harness, so Claude Code never kills the hook before ironlint can report a verdict.

## Author and review checks from inside Claude

`ironlint init --harness claude-code` installs the **`ironlint-config`** authoring skill into `.claude/skills/ironlint-config/` — the check schema, the exit-code contract, and common patterns with a fixture-test loop. Run `ironlint schema` any time to print the same guide at the terminal. `/ironlint-init` and `/ironlint-review` ship with the Claude Code **plugin** instead (see [Managing policy from inside the agent](README.md#managing-policy-from-inside-the-agent)).

The plugin layout lives in this repo at `adapters/claude-code/`. For local development, link it into Claude Code's plugin directory and restart:

```bash
ln -sf "$(pwd)/adapters/claude-code" ~/.claude/plugins/data/ironlint
```

Once IronLint is published to the plugin marketplace you can skip the symlink and run `/plugin install ironlint` instead. The plugin registers the same `PostToolUse` check, so install it *or* run `ironlint init --harness claude-code` — not both.

## When edits aren't being gated

If Claude edits a file and nothing happens, walk through these in order:

1. Confirm the hook is where Claude Code expects it. For an `init` install, that's the `PostToolUse` entry in `.claude/settings.local.json` (or `~/.claude/settings.json` with `--global`) pointing at `~/.config/ironlint/adapters/claude-code/hook.sh`, and that file must be executable. For a plugin install, the hook resolves to `${CLAUDE_PLUGIN_ROOT}/hooks/hook.sh`. A `hook.sh: No such file or directory` means the install didn't land — re-run `ironlint init --harness claude-code` or re-create the plugin symlink.
2. Confirm `ironlint --version` runs on your `PATH`.
3. Confirm `.ironlint.yml` exists in the project root.
4. Confirm the config is trusted (`ironlint init` does this; otherwise run `ironlint trust`).
5. Trace a single event end to end: `bash -x adapters/claude-code/hooks/hook.sh post-tool-use < event.json`.

For a one-shot health check, run [`ironlint doctor`](../operating/diagnostics.md). Its `claude-code` adapter row confirms the wiring without you tracing anything by hand.

## How it works

The adapter is one bash script that Claude Code calls on `PreToolUse` (matching `Edit` \| `Write` \| `MultiEdit` \| `NotebookEdit`). It only ever shells out to the `ironlint` binary and holds no policy logic of its own, so changing a check never means touching the adapter. It translates `ironlint check`'s exit codes into allow/reject per [the exit-code contract](README.md#the-exit-code-contract). The adapter hooks edits and nothing else — it does not proxy Claude's `Read`, `Grep`, or `Glob` tools.

## See also

- [Adapters overview](README.md) — the fail-open contract every adapter shares
- [Running checks](../operating/running-checks.md) — the exit codes the adapter keys off
