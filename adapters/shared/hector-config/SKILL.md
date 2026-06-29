---
name: hector-config
description: Authors, modifies, or removes checks in a hector .hector.yml. Use when the user says "add a hector check for X", "ban Y", "tighten <check-id>", "stop checking <check-id>", "remove <check-id>", "change the scope of a check", or asks how to write a hector config.
license: MIT
metadata:
  author: dynamik-dev
  version: 1.2.0
---

# Authoring hector checks

A hector policy lives in `.hector.yml` at the project root. A **check** is a file scope plus a shell command (or sequence of steps):

```yaml
checks:
  no-debug:
    files: "**/*.ts"          # glob, or a list of globs
    run: "! grep -n 'DEBUG'"  # proposed content arrives on stdin; nonzero = block
```

- `files` — the glob(s) the check watches. A bare pattern with no `/` (e.g. `*.py`) also matches at any depth.
- `run` — a shell command handed to `sh -c`. **Any nonzero exit (1–125) blocks the edit**; exit 0 passes. `126`/`127`/timeout are treated as a broken check, not a block.
- `steps` — alternative to `run`: a sequence of `{name, run}` steps, all fed the same stdin. The first nonzero step blocks.
- `on` — lifecycle events: `[write]` (default) fires per file on every agent write; `[pre-commit]` fires once before a commit (see ABI below). Use `on: [write, pre-commit]` to fire at both.
- `name` — optional human-readable label shown in block messages.

## ABI — what every check receives

- `$HECTOR_FILE` — absolute path of the single file under check (set for `write`; not set for `pre-commit`).
- `$HECTOR_FILES` — newline-joined list of all files under check (single entry for `write`; all staged files for `pre-commit`).
- `$HECTOR_ROOT` — project root (the check's cwd).
- `$HECTOR_EVENT` — `write` or `pre-commit`.
- `$HECTOR_TMPFILE` — **write only**, set only when your `run` mentions it: an absolute path to a temp file holding the proposed content, placed beside `$HECTOR_FILE` with the same extension and auto-cleaned. Use it for tools that need a real file on disk (Biome, ESLint file-mode, `tsc`, ruff) instead of stdin. Unset on `pre-commit` (files are already on disk at `$HECTOR_FILES`).
- **stdin** — proposed post-edit file content (`write`) or empty (`pre-commit`).

**Read proposed content from stdin, not from `$HECTOR_FILE`.** On harnesses that gate before the write lands (e.g. reasonix, pi), the file on disk still holds the OLD content, so reading it misses the very change you mean to check. Use `$HECTOR_FILE` to hand a tool a filename (e.g. a linter's `--stdin-filename`), never as the content source.

On block, the check's combined stdout+stderr becomes the message the agent sees, so make the command print why it blocked.

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
    run: "ruff check --quiet --stdin-filename \"$HECTOR_FILE\" -"
```

**Wrap a file-oriented linter (temp file).** Tools that won't read stdin cleanly get a real path:

```yaml
  biome-check:
    files: ["src/**/*.{ts,tsx,js,jsx}"]
    run: "npx @biomejs/biome check \"$HECTOR_TMPFILE\""
```

**Multi-step check.** Use `steps` when you want to run multiple commands in sequence — all must exit 0:

```yaml
  ts-quality:
    files: "src/**/*.ts"
    on: [pre-commit]
    steps:
      - name: typecheck
        run: "tsc --noEmit"
      - name: no-any
        run: "! grep -n ': any' $HECTOR_FILES"
```

**Multi-line scripts.** Use a YAML block scalar so newlines survive — a plain or folded (`>`) scalar collapses them and can turn the whole script into one comment that silently passes:

```yaml
  guard:
    files: "*.rs"
    run: |
      grep -q 'FORBIDDEN' && exit 1
      exit 0
```

**Pre-commit check (staged files).** Runs once before a commit, receiving all staged matching files via `$HECTOR_FILES`:

```yaml
  no-secrets:
    files: "**/*"
    on: [pre-commit]
    run: "detect-secrets scan $HECTOR_FILES"
```

## Disable a check for a file

Add `# hector-disable: <check-id>` anywhere in the file to suppress that check for the whole file:

```python
# hector-disable: no-console-log
console.log("debug only")
```

## Process

1. Read `.hector.yml` to see existing checks (if none exists, scaffold one with `hector init`).
2. Draft the check: `files` scope + a `run` command that exits nonzero to block.
3. Build two fixtures: a **dirty** file the check should block, and a **clean** one it should pass.
4. Test each by feeding the fixture's content on stdin and isolating the check:
   ```bash
   hector check --file dirty.py --check ruff-check < dirty.py ; echo "dirty exit: $?"   # expect nonzero
   hector check --file clean.py --check ruff-check < clean.py ; echo "clean exit: $?"   # expect 0
   ```
5. Verify the check exits nonzero on dirty input and 0 on clean input.
6. If both hold, write the check into `.hector.yml`.
7. Run `hector trust` to re-bless the config — edits invalidate the trust fingerprint, so checks refuse to run until you do.

## Test before write

Always test the check against a fixture BEFORE writing to `.hector.yml`. A check that doesn't exit nonzero on dirty input is worse than no check — it gives false confidence. A check that exits nonzero on clean input blocks every edit in scope.
