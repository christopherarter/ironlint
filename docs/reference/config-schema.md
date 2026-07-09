# Config schema

The full shape of `.ironlint.yml`. For a guided introduction, see [Anatomy of a check](../writing-checks/README.md); for inheritance, see [Sharing config with `extends:`](../configuring/inheritance.md).

A config has three top-level keys. Only `checks` is required:

```yaml
# .ironlint.yml
extends: ["./base.yml"]   # optional — inherit checks from parent configs
execution:                # optional — execution tuning
  timeout_secs: 30
checks:                   # required — your policy, keyed by check id
  no-console:
    files: "**/*.ts"
    run: "! grep -n 'console.log'"  # proposed content arrives on stdin
```

## Top-level

| Key | Type | Required | Notes |
|-----|------|----------|-------|
| `checks` | map of id → [check](#check) | yes | Your policy. Each key is a check id you choose. |
| `extends` | list of strings | no | Parent config paths, resolved depth-first. Local checks win on an id collision. See [Sharing config with `extends:`](../configuring/inheritance.md). |
| `execution` | block | no | See [Execution](#execution). Defaults apply when omitted. |

There is no `schema_version`, `trust`, `skip`, `rules`, `severity`, or `engine` key. A config carrying any of those is a pre-0.3 config and is rejected at load with an error pointing at this format.

## Check

A check is exactly two fields:

```yaml
checks:
  biome:
    files: ["src/**/*.ts", "src/**/*.tsx"]
    run: ".ironlint/scripts/biome.sh"
```

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `files` | string or list of strings | yes | Glob(s) the check matches. A bare string is treated as a one-element list. A pattern without `/` matches at any depth — `*.ts` is equivalent to `**/*.ts`. See [Targeting files](../configuring/targeting-files.md). |
| `run` | string | yes | A shell command, handed to `sh -c` verbatim. Any nonzero exit (1–125) blocks. See [Anatomy of a check](../writing-checks/README.md). |

`run` receives no string templating — there is no `{file}`. The path under check arrives as `$IRONLINT_FILE`, the project root as `$IRONLINT_ROOT`, the trigger as `$IRONLINT_EVENT`, and the proposed post-edit content on stdin. For `write` checks whose `run` references it, `$IRONLINT_TMPFILE` holds the absolute path to a temp file beside `$IRONLINT_FILE` containing the proposed content (same extension, auto-cleaned). `run` may be an inline command or a path to a script under `.ironlint/scripts/`; the shell makes no distinction.

`run` executes with a **scrubbed environment**, not your full shell environment: only `$PATH`, `$HOME`, locale (`$LANG`/`$LC_*`), `$TZ`, `$TMPDIR`, and the `$IRONLINT_*` vars above are set. Secrets in the parent environment (API keys, tokens) are not passed through. See [trust and the execution model](../security/trust.md#checks-run-with-a-scrubbed-environment).

## Execution

```yaml
execution:
  timeout_secs: 30
```

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `timeout_secs` | integer | `30` | Per-check wall-clock budget. A check that exceeds it is killed and reported as an internal error, never a silent pass. Clamped to a minimum of 1. |

The `IRONLINT_TIMEOUT` environment variable overrides `timeout_secs` at run time. Dispatch is sequential; there is no worker-pool tuning.

## See also

- [Anatomy of a check](../writing-checks/README.md) — what `files` and `run` do
- [Verdict JSON](verdict-json.md) — the output `ironlint check` produces
- [CLI reference](cli.md) — the commands that read this config
