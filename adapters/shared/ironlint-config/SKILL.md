---
name: ironlint-config
description: Authors, modifies, or removes checks in an ironlint .ironlint.yml. Use when the user says "add an ironlint check for X", "ban Y", "tighten <check-id>", "stop checking <check-id>", "remove <check-id>", "change the scope of a check", or asks how to write an ironlint config.
license: MIT
metadata:
  author: dynamik-dev
  version: 1.3.0
---

# Authoring ironlint checks

An ironlint policy lives in `.ironlint.yml` at the project root. A **check** is a file scope plus a shell command (or sequence of steps):

```yaml
checks:
  no-debug:
    files: "**/*.ts"          # glob, or a list of globs
    run: "! grep -n 'DEBUG'"  # proposed content arrives on stdin; nonzero = block
```

- `files` — the glob(s) the check watches. A bare pattern with no `/` (e.g. `*.py`) also matches at any depth.
- `run` — a shell command handed to `sh -c`. **Any nonzero exit (1–125) blocks the edit**; exit 0 passes. `126`/`127`/timeout are treated as a broken check, not a block.
- `steps` — alternative to `run`: a sequence of `{name?, run}` steps, all fed the same stdin. The first nonzero step blocks.
- `on` — lifecycle events: `[write]` (default) fires per file on every agent write; `[pre-commit]` fires once before a commit (see ABI below). Use `on: [write, pre-commit]` to fire at both.
- `name` — optional human-readable label. Parsed and reserved, but **not yet surfaced** in block messages or `ironlint explain` — it's a no-op today. (A `steps` entry's `name`, by contrast, *is* reported as the block's `step` field.)

## ABI — what every check receives

- `$IRONLINT_FILE` — absolute path of the single file under check (set for `write`; not set for `pre-commit`).
- `$IRONLINT_FILES` — newline-joined list of all files under check (single entry for `write`; all staged files for `pre-commit`).
- `$IRONLINT_ROOT` — project root (the check's cwd).
- `$IRONLINT_EVENT` — `write` or `pre-commit`.
- `$IRONLINT_TMPFILE` — **write only**, set only when your `run` mentions it: an absolute path to a temp file holding the proposed content, placed beside `$IRONLINT_FILE` with the same extension and auto-cleaned. Use it for tools that need a real file on disk (Biome, ESLint file-mode, `tsc`, ruff) instead of stdin. Unset on `pre-commit` (files are already on disk at `$IRONLINT_FILES`).
- `$IRONLINT_BIN` — absolute path to the `ironlint` binary running the check, so a check can shell out to it without relying on `PATH` resolution. Falls back to the bare name `ironlint` if the path can't be determined.
- `$IRONLINT_PROPOSED_MANIFEST` — optional; absolute path to a tab-separated (`file_path<TAB>content_path`) manifest of sibling proposed files in the same atomic patch. Set by some harness adapters; **absent otherwise — don't depend on it.**
- **stdin** — proposed post-edit file content (`write`) or empty (`pre-commit`).

**Read proposed content from stdin, not from `$IRONLINT_FILE`.** On harnesses that gate before the write lands (e.g. codex, pi), the file on disk still holds the OLD content, so reading it misses the very change you mean to check. Use `$IRONLINT_FILE` to hand a tool a filename (e.g. a linter's `--stdin-filename`), never as the content source.

**The check runs in a scrubbed environment.** The child process inherits only an allowlist — `PATH`, `HOME`, `LANG`, `TZ`, `TMPDIR`, and any `LC_*` — plus the `IRONLINT_*` vars above. The agent's own credentials (`ANTHROPIC_API_KEY`, `GITHUB_TOKEN`, `NPM_TOKEN`, `AWS_*`, …) are **not** inherited, so a check that shells out to a tool needing a token fails unless that token name is allowlisted. Don't write a check that assumes the parent's ambient creds.

On block, the check's combined stdout+stderr becomes the message the agent sees, so make the command print why it blocked.

## Top-level config

A minimal `.ironlint.yml` is just `checks:`. Two optional top-level keys tune it:

- `extends:` — a list of relative paths to other config files. ironlint resolves them recursively (with cycle detection); **inherited checks fill gaps where the local config doesn't define them, and local checks win on collision.** `execution` inherits the same way (nearest ancestor's value when local sets none). Trust covers the whole `extends` closure — editing any extended file invalidates the fingerprint.

  ```yaml
  extends:
    - ../shared/.ironlint.yml
  checks: {}
  ```

- `execution.timeout_secs` — per-check wall-clock (default `30`). A check that exceeds it is killed and reported as InternalError, never a silent pass. Override at run time with the `IRONLINT_TIMEOUT` env var (seconds).

  ```yaml
  execution:
    timeout_secs: 60
  ```

## Check patterns

**Ban a pattern (grep, reads stdin).** With nonzero-blocks, `! grep` is the natural idiom — grep exits 0 on a match (which `!` flips to 1, blocking) and exits 1 when clean (which `!` flips to 0, passing):

```yaml
  no-console-log:
    files: ["src/**/*.ts", "src/**/*.tsx"]
    run: "! grep -nE 'console\\.log\\('"
```

**Wrap a linter (stdin).** Feed the proposed content to a linter. Most linters exit nonzero on findings, which blocks directly:

```yaml
  ruff-check:
    files: ["**/*.py"]
    run: "ruff check --quiet --stdin-filename \"$IRONLINT_FILE\" -"
```

**Wrap a file-oriented linter (temp file).** Tools that won't read stdin cleanly get a real path:

```yaml
  biome-check:
    files: ["src/**/*.{ts,tsx,js,jsx}"]
    run: "npx @biomejs/biome check \"$IRONLINT_TMPFILE\""
```

**Limitation:** `$IRONLINT_TMPFILE` has a synthetic name (`ironlint-tmp-…`), so tools that select behaviour by *filename glob* — e.g. ESLint `overrides` scoped to `*.test.ts`, or Biome `include`/`ignore` patterns — won't match it the way they'd match the real file. Language detection (by extension) and nearest-config resolution (the temp file sits beside `$IRONLINT_FILE`) do work. When a tool needs the real filename for its config, pass `$IRONLINT_FILE` for that and `$IRONLINT_TMPFILE` only for the content.

**Multi-step check.** Use `steps` when you want to run multiple commands in sequence — all must exit 0:

```yaml
  ts-quality:
    files: "src/**/*.ts"
    on: [pre-commit]
    steps:
      - name: typecheck
        run: "tsc --noEmit"
      - name: no-any
        run: "! grep -n ': any' $IRONLINT_FILES"
```

**Multi-line scripts.** Use a YAML block scalar so newlines survive — a plain or folded (`>`) scalar collapses them and can turn the whole script into one comment that silently passes:

```yaml
  guard:
    files: "*.rs"
    run: |
      grep -q 'FORBIDDEN' && exit 1
      exit 0
```

**Pre-commit check (staged files).** Runs once before a commit, receiving all staged matching files via `$IRONLINT_FILES`:

```yaml
  no-secrets:
    files: "**/*"
    on: [pre-commit]
    run: "detect-secrets scan $IRONLINT_FILES"
```

## Disable a check for a file

Add `# ironlint-disable: <check-id>` anywhere in the file to suppress that check for the whole file:

```python
# ironlint-disable: no-console-log
console.log("debug only")
```

## Process

1. Read `.ironlint.yml` to see existing checks (if none exists, scaffold one with `ironlint init`).
2. Draft the check: `files` scope + a `run` command that exits nonzero to block.
3. Build two fixtures: a **dirty** file the check should block, and a **clean** one it should pass.
4. Test each by feeding the fixture's content on stdin and isolating the check:
   ```bash
   ironlint check --file dirty.py --check ruff-check < dirty.py ; echo "dirty exit: $?"   # expect nonzero
   ironlint check --file clean.py --check ruff-check < clean.py ; echo "clean exit: $?"   # expect 0
   ```
5. Verify the check exits nonzero on dirty input and 0 on clean input.
6. If both hold, write the check into `.ironlint.yml`.
7. Run `ironlint trust` to re-bless the config — edits invalidate the trust fingerprint, so checks refuse to run until you do.

## Test before write

Always test the check against a fixture BEFORE writing to `.ironlint.yml`. A check that doesn't exit nonzero on dirty input is worse than no check — it gives false confidence. A check that exits nonzero on clean input blocks every edit in scope.
