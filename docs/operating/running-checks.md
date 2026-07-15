# Running checks

`ironlint check` runs your checks against a file and returns a verdict. It's what your adapter calls on every edit, and what you run by hand to test a check.

```bash
ironlint check --file src/auth.rs
```

IronLint loads `.ironlint.yml`, confirms the config is trusted, then runs every check whose `files` globs match the path — one `run` invocation per matching file. It reads only each check's exit code, folds them into a single verdict, and prints it.

## Choosing what to check

| Input | Checks |
|------|--------|
| `--file <path>` | A single file on disk. |
| `--diff <path>` | A unified diff; each changed file is checked. |
| `--content <string\|->` | Proposed post-edit content instead of reading `--file` from disk — pass `-` to read it from stdin. Requires `--file`; conflicts with `--diff`. |
| no `--file` or `--diff` | A repository sweep. IronLint walks the config directory, respects `.gitignore`, and skips hidden directories. Checks with only `on: [write]` run once per matching file; any check that includes `pre-commit` (including `on: [write, pre-commit]`) runs once over its matching file set. |

`--content` evaluates a *proposed* edit before it lands on disk — the case an adapter hits when it checks an agent's write before committing it. The proposed content reaches every matching check the same way on-disk content does: on **stdin**. There is no engine/on-disk split — a check sees the bytes you pass, whether they came from disk or `--content`.

By default, `--file` and `--diff` dispatch the `write` lifecycle. Pass
`--event pre-commit` when you need the batched lifecycle for that selected
input. You cannot pass `--event` with a bare repository sweep because the
sweep runs each check according to its `on:` list.

## Exit codes

The exit code is the contract — adapters and CI branch on it:

| Code | Meaning |
|------|---------|
| `0` | **Pass** — no check blocked and none crashed. |
| `1` | **Config or load error** — parse failure, missing file, unknown `--check`. No verdict is produced. |
| `2` | **Block** — at least one check blocked (exited nonzero). |
| `3` | **Internal error** — at least one check crashed (command not found, not executable, timed out, or killed by a signal). |
| `4` | **Untrusted config or checks** — run `ironlint trust`. Emitted by the trust gate *before* the engine loads; no verdict is produced. |

There is no warn tier. `0` and `2` are the normal pass/block signals. `1` means *fix your config*; `3` means *a check couldn't run* — distinct from *a check found a problem*; `4` means *trust the config*. A check blocks by exiting nonzero (`1`–`125`); `0` is the only pass. `126`/`127`/a signal death/a timeout are internal errors, not passes — so a broken check is never a silent pass.

## Fail-open vs. fail-closed on internal errors

Exit `3` is an open question: a check couldn't run, so IronLint can't say pass or block. Adapters **fail-open** on `3` by default — the edit is allowed, because an unrelated problem (a script that failed to spawn, say) shouldn't block an agent's work.

To flip that and treat internal errors as blocking, set:

```bash
export IRONLINT_FAIL_CLOSED_ON_INTERNAL=1
```

Use fail-closed where a skipped check is unacceptable — a CI pipeline where a check silently not running would let a violation through.

Exit `4` is the opposite default. An untrusted config is never silently un-gated: adapters surface it loudly, and every pre-write adapter treats it as fail-closed and blocks the tool call outright. Bless the config with `ironlint trust`; see [The trust store](../security/trust.md).

## Other useful flags

| Flag | Effect |
|------|--------|
| `--config <path>` | Load a config other than `.ironlint.yml`. |
| `--check <id>` | Run only this check. Repeatable; multiple flags are OR'd. |
| `--force` | Run a named `--check` against `--file` even when the path is outside that check's `files:` scope. It does not bypass lifecycle or `ironlint-disable:` directives. |
| `--require-match` | Make a file that matches no checks fail instead of reporting a visible no-match pass. Use it in CI to catch stale globs. |
| `--allow-external-paths` | Allow a file outside the config directory. Keep the default unless you intentionally check a file outside the project. |

For JSON output (`--format json`) and the complete flag list, see the [CLI reference](../reference/cli.md) and [Verdict JSON](../reference/verdict-json.md).

## See also

- [Verdict JSON](../reference/verdict-json.md) — the machine-readable verdict and exit codes
- [Inspecting your config](inspecting-config.md) — read-only commands that never run a check
- [Diagnostics](diagnostics.md) — `ironlint doctor` when checks behave unexpectedly
