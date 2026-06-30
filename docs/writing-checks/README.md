# Anatomy of a check

A check is one policy: something your agent must not do, plus a command that detects it. You write checks under the `checks:` map in `.hector.yml`, keyed by a check id you choose.

Here is a complete check:

```yaml
checks:
  no-console:
    files: "src/**/*.ts"
    run: "! grep -n 'console.log'"
```

That's the whole surface. A check has exactly two fields:

| Field | What it does |
|-------|--------------|
| `files` | The glob (or list of globs) selecting which files this check watches. A bare pattern without `/` matches at any depth — `*.ts` is the same as `**/*.ts`. See [Targeting files](../configuring/targeting-files.md). |
| `run` | A shell command, handed to `sh -c` verbatim. Hector reads only its exit code. |

## The exit-code contract

Hector runs `run` once per matching file and looks at nothing but the exit code:

| Exit code | Outcome |
|-----------|---------|
| `0` | **Pass** — the edit is allowed. |
| `1`–`125` | **Block** — the edit is rejected. |
| `126` / `127` / killed by signal / timeout | **Internal error** — the check is broken; it's reported, never a silent pass. |

Any nonzero exit (1–125) blocks; `0` passes. A tool that exits nonzero when it finds problems — `eslint`, `phpstan` — blocks by default. The common shapes:

```sh
phpstan analyse "$HECTOR_FILE"                    # block if the tool fails
grep -q 'TODO' && exit 1 || true   # block if a pattern is present
! grep -q 'TODO'                   # same, in negated form
```

## What the check's output becomes

When a check exits nonzero (1–125), Hector takes its combined stdout and stderr, trims them, and uses that verbatim as the block message the agent sees. Print whatever helps the agent fix the problem — a `file:line`, the failing rule, a suggested change. If the check prints nothing, the message is `"<check-id> blocked"`.

## The ABI: what every check receives

Hector hands each check the same four things. Nothing is spliced into the command text, so a path with spaces or shell metacharacters can't break out.

| Channel | Value |
|---------|-------|
| `$HECTOR_FILE` | Absolute path to the file under check. |
| `$HECTOR_ROOT` | Project root — also the check's working directory. |
| `$HECTOR_EVENT` | What triggered the check: `write` or `pre-commit`. |
| `$HECTOR_TMPFILE` | **write only** — set only when your `run` references it: absolute path to a temp file beside `$HECTOR_FILE` holding the proposed content (same extension, auto-cleaned). Use for tools that won't read stdin. Unset on `pre-commit`. |
| stdin | The proposed post-edit content of the file (may be empty). |

There is no `{file}` token. The path travels only as `$HECTOR_FILE`.

### Reading the proposed edit vs. the file on disk

This is the one subtlety worth understanding. When an adapter intercepts an edit *before* it lands, the new bytes arrive on **stdin**, while `$HECTOR_FILE` may still point at the old on-disk content. So:

- To check the **proposed** content, read stdin — `biome check --stdin-file-path "$HECTOR_FILE"`.
- To check the **on-disk tree** — a tool that needs a real, consistent file tree, like a dependency-graph check — ignore stdin and read `$HECTOR_FILE` or scan `$HECTOR_ROOT`.

Pick the one your tool needs. [Check recipes](recipes.md) shows both.

## Inline command or script file

`run` can be an inline command (as above) or a path to a script:

```yaml
checks:
  biome:
    files: ["src/**/*.ts", "src/**/*.tsx"]
    run: ".hector/gates/biome.sh"
```

The shell makes no distinction. Keep a one-liner inline; move anything longer into `.hector/gates/` so it's readable and version-controlled. Scripts under `.hector/gates/` are covered by `hector trust`, so editing one re-triggers a blessing.

## See also

- [Check recipes](recipes.md) — worked checks for common policies
- [Targeting files](../configuring/targeting-files.md) — the `files:` globs
- [Config schema](../reference/config-schema.md) — the exhaustive field reference
