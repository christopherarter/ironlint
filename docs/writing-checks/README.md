# Anatomy of a check

A check is one policy: something your agent must not do, plus a command that detects it. You write checks under the `checks:` map in `.ironlint.yml`, keyed by a check id you choose.

Here is a complete check:

```yaml
checks:
  no-console:
    files: "src/**/*.ts"
    run: "! grep -n 'console.log'"
    on: [write, pre-commit]   # optional — default [write]
    name: "no console.log"    # optional — human-readable label
```

A check is `files` plus a command — either `run` (a single shell command) or `steps` (a sequential list). Two optional fields round it out:

| Field | What it does |
|-------|--------------|
| `files` | The glob (or list of globs) selecting which files this check watches. A bare pattern without `/` matches at any depth — `*.ts` is the same as `**/*.ts`. See [Targeting files](../configuring/targeting-files.md). |
| `run` | A shell command, handed to `sh -c` verbatim. IronLint reads only its exit code. (One of `run` / `steps`.) |
| `steps` | A sequence of `{ name?, run }` steps, all fed the same stdin. The first nonzero step blocks. `run:` is sugar for a single step. |
| `on` | Lifecycle: `[write]` (default — per file on every edit), `[pre-commit]` (once over the selected matching file set), or `[write, pre-commit]`. |
| `name` | Human-readable label. Parsed and reserved; not yet surfaced in output. |

## The exit-code contract

IronLint runs `run` once per matching file and looks at nothing but the exit code:

| Exit code | Outcome |
|-----------|---------|
| `0` | **Pass** — the edit is allowed. |
| `1`–`125` | **Block** — the edit is rejected. |
| `126` / `127` / killed by signal / timeout | **Internal error** — the check is broken; it's reported, never a silent pass. |

Any nonzero exit (1–125) blocks; `0` passes. A tool that exits nonzero when it finds problems — `eslint`, `phpstan` — blocks by default. The common shapes:

```sh
phpstan analyse "$IRONLINT_FILE"                    # block if the tool fails
grep -q 'TODO' && exit 1 || true   # block if a pattern is present
! grep -q 'TODO'                   # same, in negated form
```

## What the check's output becomes

When a check exits nonzero (1–125), IronLint takes its combined stdout and stderr, trims them, and uses that verbatim as the block message the agent sees. Print whatever helps the agent fix the problem — a `file:line`, the failing rule, a suggested change. If the check prints nothing, the message is `"<check-id> blocked"`.

## The ABI: what every check receives

IronLint hands each check the same four things. Nothing is spliced into the command text, so a path with spaces or shell metacharacters can't break out.

| Channel | Value |
|---------|-------|
| `$IRONLINT_FILE` | Absolute path to the file under check (set for `write`; not set for `pre-commit`). |
| `$IRONLINT_FILES` | Newline-joined list of all selected files (single entry for `write`; the matching file set for `pre-commit`). |
| `$IRONLINT_ROOT` | Project root — also the check's working directory. |
| `$IRONLINT_EVENT` | What triggered the check: `write` or `pre-commit`. |
| `$IRONLINT_BIN` | Absolute path to the `ironlint` binary, so a check can invoke it without `PATH` resolution. |
| `$IRONLINT_TMPFILE` | **write only** — set only when your `run` references it: absolute path to a temp file beside `$IRONLINT_FILE` holding the proposed content (same extension, auto-cleaned). Use for tools that won't read stdin. Unset on `pre-commit`. |
| stdin | The proposed post-edit content of the file (may be empty). |

`$IRONLINT_PROPOSED_MANIFEST` is an optional tab-separated (`file_path<TAB>content_path`) manifest of sibling proposed files in the same atomic patch. It is set by some adapters and absent otherwise, so checks that use it should handle its absence.

There is no `{file}` token. The path travels only as `$IRONLINT_FILE`.

**The environment is scrubbed, not inherited.** A check does not see your full shell environment — IronLint runs it with only `$PATH`, `$HOME`, locale (`$LANG`/`$LC_*`), `$TZ`, `$TMPDIR`, and the `$IRONLINT_*` vars above. Secrets sitting in the parent process's environment (`$ANTHROPIC_API_KEY`, `$GITHUB_TOKEN`, `$AWS_*`, etc.) are not passed through. If a check genuinely needs a credential, read it from a file or a secrets manager inside the check itself — don't rely on ambient env vars. See [trust and the execution model](../security/trust.md#checks-run-with-a-scrubbed-environment).

### Reading the proposed edit vs. the file on disk

This is the one subtlety worth understanding. When an adapter intercepts an edit *before* it lands, the new bytes arrive on **stdin**, while `$IRONLINT_FILE` may still point at the old on-disk content. So:

- To check the **proposed** content, read stdin — `biome check --stdin-file-path "$IRONLINT_FILE"`.
- To check the **on-disk tree** — a tool that needs a real, consistent file tree, like a dependency-graph check — ignore stdin and read `$IRONLINT_FILE` or scan `$IRONLINT_ROOT`.

Pick the one your tool needs. [Check recipes](recipes.md) shows both.

## Inline command or script file

`run` can be an inline command (as above) or a path to a script:

```yaml
checks:
  biome:
    files: ["src/**/*.ts", "src/**/*.tsx"]
    run: ".ironlint/scripts/biome.sh"
```

The shell makes no distinction. Keep a one-liner inline; move anything longer into `.ironlint/scripts/` so it's readable and version-controlled. Scripts under `.ironlint/scripts/` are covered by `ironlint trust`, so editing one re-triggers a blessing.

## See also

- [Check recipes](recipes.md) — worked checks for common policies
- [Targeting files](../configuring/targeting-files.md) — the `files:` globs
- [Config schema](../reference/config-schema.md) — the exhaustive field reference
