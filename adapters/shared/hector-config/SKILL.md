---
name: hector-config
description: Authors, modifies, or removes checks in a hector .hector.yml. Use when the user says "add a hector check for X", "ban Y", "tighten <check-id>", "stop checking <check-id>", "remove <check-id>", "change the scope of a check", or asks how to write a hector config.
license: MIT
metadata:
  author: dynamik-dev
  version: 1.1.0
---

# Authoring hector checks

A hector policy lives in `.hector.yml` at the project root. A **check** is exactly
two fields — there are no engines, severities, or output modes:

```yaml
checks:
  no-debug:
    files: "**/*.ts"          # glob, or a list of globs
    run: "! grep -n 'DEBUG' || exit 2"   # grep reads the proposed content on stdin
```

- `files` — the glob(s) the check watches. A bare pattern with no `/` (e.g.
  `*.py`) also matches at any depth.
- `run` — a shell command handed to `sh -c`. The check **owns the verdict via its
  exit code**: exit `2` blocks the edit; `0` (and any other non-`2` code up to
  125) passes. `126`/`127`/timeout are treated as a broken check, not a block.
- **Read the proposed content from stdin, not from `$HECTOR_FILE`.** Stdin
  carries the post-edit content on every harness. `$HECTOR_FILE` is only the
  absolute path: on harnesses that gate *before* the write lands (e.g. reasonix,
  pi), the file on disk still holds the OLD content, so reading it misses the
  very change you mean to check. Use `$HECTOR_FILE` to hand a tool a filename
  (e.g. a linter's `--stdin-filename`), never as the content source.
  `$HECTOR_ROOT` (project root) and `$HECTOR_EVENT`
  (`write`/`pre-commit`) are also set. There is no path
  templating — the path travels only as `$HECTOR_FILE`, never spliced into
  `run`.
- On block, the check's combined stdout+stderr becomes the message the agent
  sees, so make the command print why it blocked.

## Check patterns

**Ban a pattern (grep).** Block when a forbidden string appears. `grep` exits `0`
on a match, `1` when clean, `≥2` on error — map those to the check contract:

```yaml
  no-console-log:
    files: ["src/**/*.ts", "src/**/*.tsx"]
    run: "grep -nE 'console\\.log\\(' -; case $? in 0) exit 2;; 1) exit 0;; *) exit $?;; esac"
```

**Wrap a linter (stdin).** Feed the proposed content to a linter so the check runs
pre-write. Most linters exit non-zero on findings; remap that to `2` to block:

```yaml
  ruff-check:
    files: ["**/*.py"]
    run: "ruff check --quiet --stdin-filename \"$HECTOR_FILE\" - || exit 2"
```

**Multi-line scripts.** Use a YAML block scalar so newlines survive — a plain or
folded (`>`) scalar collapses them and can turn the whole script into one comment
that silently passes:

```yaml
  guard:
    files: "*.rs"
    run: |
      grep -q 'FORBIDDEN' && exit 2
      exit 0
```

## Process

1. Read `.hector.yml` to see existing checks (if none exists, scaffold one with
   `hector init`).
2. Draft the check: `files` scope + a `run` command that exits `2` to block.
3. Build two fixtures: a **dirty** file the check should block, and a **clean** one
   it should pass.
4. Test each by feeding the fixture's content on stdin and isolating the check:
   ```bash
   hector check --file dirty.py --content - --check ruff-check < dirty.py ; echo "dirty exit: $?"   # expect 2
   hector check --file clean.py --content - --check ruff-check < clean.py ; echo "clean exit: $?"   # expect 0
   ```
5. Verify the check exits `2` on dirty input and `0` on clean input.
6. If both hold, write the check into `.hector.yml`.
7. Run `hector trust` to re-bless the config — edits invalidate the trust
   fingerprint, so checks refuse to run until you do.

## Test before write

Always test the check against a fixture BEFORE writing to `.hector.yml`. A check
that doesn't exit `2` on dirty input is worse than no check — it gives false
confidence. A check that exits `2` on clean input blocks every edit in scope.
