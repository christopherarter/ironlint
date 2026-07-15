# Check recipes

Worked checks for policies you'll actually want. Each is a complete `.ironlint.yml` entry; drop it under `checks:` and adjust the id, glob, and command. For the rules these rely on — the exit-code contract and the ABI — see [Anatomy of a check](README.md).

## Ban a pattern with grep

The simplest check: block an edit when a forbidden string appears.

```yaml
checks:
  no-focused-tests:
    files: "**/*.test.ts"
    run: "! grep -n '\\.only('"  # proposed content arrives on stdin
```

`grep` exits `0` on a match, so `! grep …` flips that to a failure on a hit, and the nonzero exit blocks the edit. `-n` makes grep include line numbers in its output, which becomes the message the agent sees.

## Run a linter over the proposed content

A linter that reads stdin can check the new bytes before they land. Move it into a script so it stays readable:

```yaml
checks:
  biome:
    files: ["src/**/*.ts", "src/**/*.tsx"]
    run: ".ironlint/scripts/biome.sh"
```

```sh
# .ironlint/scripts/biome.sh
#!/usr/bin/env sh
# Lint the proposed content, which IronLint delivers on stdin.
biome check --stdin-file-path "$IRONLINT_FILE"
```

`biome check` exits non-zero when it finds problems; the nonzero exit blocks the edit. Because it reads stdin, it sees the edit the agent is *proposing*, not whatever is currently on disk. Make the script executable: `chmod +x .ironlint/scripts/biome.sh`.

## Run a file-oriented linter (temp file)

Some tools refuse to read stdin and require a real file path. Reference `$IRONLINT_TMPFILE` in your `run` and IronLint writes the proposed content to a temp file beside `$IRONLINT_FILE` with the same extension, then removes it after the check:

```yaml
checks:
  biome-file:
    files: ["src/**/*.ts", "src/**/*.tsx"]
    run: "npx @biomejs/biome check \"$IRONLINT_TMPFILE\""
```

**Limitation:** `$IRONLINT_TMPFILE` has a synthetic name (`ironlint-tmp-…`), so filename-glob configuration in tools — ESLint `overrides` scoped to `*.test.ts`, Biome `include`/`ignore` patterns — may not match it. Language detection by extension and nearest-config resolution (the temp file sits beside `$IRONLINT_FILE`) work correctly. When a tool needs the real filename for its config lookup, pass `$IRONLINT_FILE` for that argument and `$IRONLINT_TMPFILE` for the content.

## Run a whole-tree tool

Some checks need a real, consistent file tree — a dependency-graph rule, a typechecker that resolves imports. These ignore stdin and read the on-disk tree from `$IRONLINT_ROOT`:

```yaml
checks:
  depcruise:
    files: "src/**/*.ts"
    run: "npx depcruise --validate .dependency-cruiser.js src"
```

The check's working directory is `$IRONLINT_ROOT`, so a relative path like `src` resolves against the project root. In a batch run this re-runs once per changed file, which is redundant but correct — the check is idempotent.

## Ask a model to judge

Because a check is just a command that exits nonzero, a model can be the judge:

```yaml
checks:
  no-secrets:
    files: "**/*"
    run: ".ironlint/scripts/secret-scan.sh"
```

```sh
# .ironlint/scripts/secret-scan.sh
#!/usr/bin/env sh
content=$(cat)
verdict=$(printf '%s' "$content" | claude -p "Reply BLOCK if this file contains a hardcoded secret, otherwise PASS.")
case "$verdict" in
  *BLOCK*) echo "Possible hardcoded secret — move it to an environment variable." >&2; exit 1 ;;
  *)       exit 0 ;;
esac
```

`cat` reads the proposed content from stdin. The check decides on its own judgement and exits nonzero to block, with no special support from IronLint.

## Block only on a specific event

`$IRONLINT_EVENT` tells a check how it was triggered, so you can be strict at commit time but lenient on live edits:

```yaml
checks:
  tests-pass-precommit:
    files: "src/**/*.rs"
    run: "[ \"$IRONLINT_EVENT\" = pre-commit ] || exit 0; cargo test -q"
```

On any event other than `pre-commit`, the check exits `0` immediately. When you invoke the pre-commit lifecycle, it runs the tests and blocks if they fail.

## See also

- [Anatomy of a check](README.md) — the contract these recipes rely on
- [Targeting files](../configuring/targeting-files.md) — the `files:` globs
- [The trust store](../security/trust.md) — why editing a check script re-triggers a blessing
