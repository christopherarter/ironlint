---
name: hector-init
description: Bootstraps a project's .hector.yml by detecting the tech stack from manifest files, wrapping existing linters as gates, and generating a baseline config. Use when user says "init hector", "set up hector", "bootstrap hector config", "create hector gates", "hector init", or asks to create or generate a hector configuration.
metadata:
  author: dynamik-dev
  version: 0.2.0
  category: workflow-automation
  tags: [linting, code-quality, config-generation, stack-detection]
---

# Hector Init

Generate a baseline `.hector.yml` by detecting the stack and wrapping installed
linters as gates. A gate is two fields — `files` (a glob or list) and `run` (a
shell command that exits `2` to block); there are no engines or severities.

This skill is user-driven. Do not silently install tools or add gates. Every step
below is a proposal the user accepts or declines.

## Step 1: Run `hector init`

`hector init` detects the stack from manifest files, writes a starter
`.hector.yml`, and trusts it for you:

| Manifest | Stack |
|---|---|
| `Cargo.toml` | Rust |
| `package.json` | Node |
| `pyproject.toml` or `setup.py` | Python |
| (none) | Generic |

```bash
hector init
```

This creates `.hector.yml` with one or two starter gates for the detected stack
(and, outside this Claude session, can also wire hooks into other agents — not
needed here, the PostToolUse hook is already running). Review the generated gates.

## Step 2: Wrap the project's linters as gates

For each linter the user has installed (ruff, biome, eslint, tsc, phpstan,
clippy, …) that runs per file, propose a gate that feeds the proposed content on
stdin and remaps a non-zero linter exit to `2`:

```yaml
gates:
  ruff-check:
    files: ["**/*.py"]
    run: "ruff check --quiet --stdin-filename \"$HECTOR_FILE\" - || exit 2"
```

Test each candidate against a sample file before adding it (see `/hector-config`
for the fixture loop). Only add gates that pass on clean input and block on dirty
input. Skip repo-wide tools that aren't per-file (e.g. `cargo clippy`) — they
don't map to a per-file gate; suggest running them as a pre-push step instead.

## Step 3: Trust the config

`hector init` already trusted the config it scaffolded. If you hand-edit
`.hector.yml` (Step 2), re-bless it:

```bash
hector trust
```

This records a sha256 of the config (and any files it `extends:`/`.hector/gates/`)
in the out-of-repo trust store at `~/.config/hector/trust.json` — it does **not**
write into `.hector.yml`. Any later edit invalidates the fingerprint, and
`hector check` refuses to run until you re-trust.

## Step 4: Verify

Edit any in-scope file. The PostToolUse hook runs hector and either passes (clean)
or blocks (with the gate's message). See the `hector` skill for how to read a
block verdict.

## Notes

- If `.hector.yml` already exists, do not overwrite it — `hector init` leaves an
  existing config untouched. Propose edits via `/hector-config` instead.
- There is no migration from older formats; hector rejects a pre-0.3 config
  (`schema_version:`/`rules:`) outright. Write gates fresh.
- Telemetry lands at `.hector/log.jsonl`, one record per check with a per-gate
  breakdown. The `/hector-review` skill consumes it.
