---
name: ironlint-init
description: Bootstraps a project's .ironlint.yml by detecting the tech stack from manifest files, wrapping existing linters as checks, and generating a baseline config. Use when user says "init ironlint", "set up ironlint", "bootstrap ironlint config", "create ironlint checks", "ironlint init", or asks to create or generate an ironlint configuration.
metadata:
  author: dynamik-dev
  version: 0.2.0
  category: workflow-automation
  tags: [linting, code-quality, config-generation, stack-detection]
---

# IronLint Init

Generate a baseline `.ironlint.yml` by detecting the stack and wrapping installed
linters as checks. A check is two fields — `files` (a glob or list) and `run` (a
shell command that exits nonzero to block); there are no engines or severities.

This skill is user-driven. Do not silently install tools or add checks. Every step
below is a proposal the user accepts or declines.

## Step 1: Run `ironlint init`

`ironlint init` detects the stack from manifest files, writes a starter
`.ironlint.yml`, and trusts it for you:

| Manifest | Stack |
|---|---|
| `Cargo.toml` | Rust |
| `package.json` | Node |
| `pyproject.toml` or `setup.py` | Python |
| (none) | Generic |

```bash
ironlint init
```

This creates `.ironlint.yml` with one or two starter checks for the detected stack
(and, outside this Claude session, can also wire hooks into other agents — not
needed here, the PreToolUse hook is already running). Review the generated checks.

## Step 2: Wrap the project's linters as checks

For each linter the user has installed (ruff, biome, eslint, tsc, phpstan,
clippy, …) that runs per file, propose a check that feeds the proposed content on
stdin; any nonzero exit blocks:

```yaml
checks:
  ruff-check:
    files: ["**/*.py"]
    run: "ruff check --quiet --stdin-filename \"$IRONLINT_FILE\" -"
```

Test each candidate against a sample file before adding it (see `/ironlint-config`
for the fixture loop). Only add checks that pass on clean input and block on dirty
input. Skip repo-wide tools that aren't per-file (e.g. `cargo clippy`) — they
don't map to a per-file check; suggest running them as a pre-push step instead.

## Step 3: Trust the config

`ironlint init` already trusted the config it scaffolded. If you hand-edit
`.ironlint.yml` (Step 2), re-bless it:

```bash
ironlint trust
```

This records a sha256 of the config (and any files it `extends:`/`.ironlint/gates/`)
in the out-of-repo trust store at `~/.config/ironlint/trust.json` — it does **not**
write into `.ironlint.yml`. Any later edit invalidates the fingerprint, and
`ironlint check` refuses to run until you re-trust.

## Step 4: Verify

Edit any in-scope file. The PreToolUse hook runs ironlint and either passes (clean)
or blocks (with the check's message). See the `ironlint` skill for how to read a
block verdict.

## Notes

- If `.ironlint.yml` already exists, do not overwrite it — `ironlint init` leaves an
  existing config untouched. Propose edits via `/ironlint-config` instead.
- There is no migration from older formats; ironlint rejects a pre-0.3 config
  (`schema_version:`/`rules:`) outright. Write checks fresh.
- Telemetry lands at `.ironlint/log.jsonl`, one record per check with a per-check
  breakdown. The `/ironlint-review` skill consumes it.
