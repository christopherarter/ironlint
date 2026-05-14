# `hector show-resolved-config`

Read-only inspection command. Prints the post-`extends:` merged rule
set so authors can confirm what their actual config looks like after
inheritance.

```bash
hector show-resolved-config [--config .hector.yml] [--format tsv|yaml|json]
```

Exit codes: `0` on success; `1` on config error (missing file, parse
failure, unsupported schema). Never `2` — this command does not run
rules.

This command does **not** verify the trust fingerprint. Operators
typically reach for it precisely when debugging an as-yet-unsigned
config, so trust enforcement would defeat the purpose. The command is
read-only and never executes a `script:` rule.

## Origin attribution

Every rule in the output is annotated with the canonical path of the
file it was *defined in* — your local `.hector.yml`, an
`extends:`-referenced parent, or a deeper transitive ancestor. When a
rule id collides between the local file and an inherited one, the
local definition wins (matching `extends::resolve` semantics) and the
origin reflects that.

## Output: TSV (default)

Columns, in order, separated by a single tab; one rule per line; rows
sorted by rule id; no header row.

| # | Column     | Notes |
|---|------------|-------|
| 1 | `id`       | Rule id from the merged config. |
| 2 | `engine`   | One of `script`, `ast`, `semantic`, `session`. |
| 3 | `severity` | One of `error`, `warning`. |
| 4 | `scope`    | Comma-separated list of glob patterns. No tabs inside the cell. |
| 5 | `fix_hint` | Empty cell when the rule has no fix_hint (column count is preserved). |
| 6 | `origin`   | Canonical filesystem path of the file that defined the rule. |

Greppable / cuttable:

```bash
hector show-resolved-config | cut -f1,2,6     # ids + engine + origin
hector show-resolved-config | grep semantic   # all semantic rules
```

## Output: YAML (`--format yaml`)

Canonical `serde_yaml` rendering of a view that *intentionally* omits
two fields from the live `Config` shape:

- `trust:` is per-config-file. The post-merge view has no single source
  file to fingerprint, so emitting one would mislead.
- `extends:` is already consumed by the merge.

Each rule entry is preceded by a `# origin: <path>` comment line so
the inheritance source is visible in the rendered YAML.

```yaml
schema_version: 2
rules:
  # origin: /work/repo/parent.yml
  inherited:
    description: "from parent"
    engine: script
    scope:
    - "*.txt"
    severity: warning
    script: "true"
  # origin: /work/repo/.hector.yml
  local-only:
    description: "only in child"
    engine: script
    scope:
    - "*.md"
    severity: warning
    script: "true"
```

## Output: JSON (`--format json`)

Pretty-printed `serde_json` rendering of the same view as YAML. Rules
are sorted by id (the `BTreeMap` keys ordering is preserved through
`serde_json::Map`'s insertion order). Each rule object carries an
`origin` field with the canonical defining-file path.

```json
{
  "schema_version": 2,
  "rules": {
    "inherited": {
      "description": "from parent",
      "engine": "script",
      "scope": ["*.txt"],
      "severity": "warning",
      "script": "true",
      "origin": "/work/repo/parent.yml"
    },
    "local-only": {
      "description": "only in child",
      "engine": "script",
      "scope": ["*.md"],
      "severity": "warning",
      "script": "true",
      "origin": "/work/repo/.hector.yml"
    }
  }
}
```

## Stability

These three output shapes are a public contract. TSV column order, the
YAML field set (sans `trust:` / `extends:`), and the JSON object
structure all freeze with this command. Breaking changes go through a
versioned `--format` value (e.g. `--format json-v2`).
