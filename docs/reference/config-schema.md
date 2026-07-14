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

A check is `files` plus a command (either `run` or `steps`), with two optional fields:

```yaml
checks:
  biome:
    files: ["src/**/*.ts", "src/**/*.tsx"]
    run: ".ironlint/scripts/biome.sh"
    on: [write, pre-commit]   # optional — default [write]
    name: "Biome"             # optional — human-readable label
```

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `files` | string or list of strings | yes | Glob(s) the check matches. A bare string is treated as a one-element list. A pattern without `/` matches at any depth — `*.ts` is equivalent to `**/*.ts`. See [Targeting files](../configuring/targeting-files.md). |
| `run` | string | one of `run`/`steps` | A shell command, handed to `sh -c` verbatim. Any nonzero exit (1–125) blocks. See [Anatomy of a check](../writing-checks/README.md). |
| `steps` | list of `{name?, run}` | one of `run`/`steps` | A sequence of steps, all fed the same stdin. The first nonzero step blocks. `run: "cmd"` is sugar for a single-step `steps`. |
| `on` | list of `write` / `pre-commit` | no (default `[write]`) | When the check fires. `write`: once per matching file, proposed content on stdin. `pre-commit`: once over the whole matching set, `$IRONLINT_FILES` populated, stdin empty. Use `[write, pre-commit]` for both. |
| `name` | string | no | Human-readable label. Parsed and reserved; not yet surfaced in output. |

`run` receives no string templating — there is no `{file}`. The path under check arrives as `$IRONLINT_FILE` (single file; unset on `pre-commit`), the full file set as `$IRONLINT_FILES`, the project root as `$IRONLINT_ROOT`, the trigger as `$IRONLINT_EVENT`, and the proposed post-edit content on stdin. For `write` checks whose `run` references it, `$IRONLINT_TMPFILE` holds the absolute path to a temp file beside `$IRONLINT_FILE` containing the proposed content (same extension, auto-cleaned). `$IRONLINT_BIN` is the absolute path to the running IronLint binary available to any check (so a check can shell out to it without `PATH` resolution). `$IRONLINT_PROPOSED_MANIFEST` is an optional sibling-proposal manifest available to any check during an atomic patch; it is an absolute path to tab-separated `file_path\tcontent_path` lines and is absent otherwise. `run` may be an inline command or a path to a script under `.ironlint/scripts/`; the shell makes no distinction.

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
