# Quickstart

Create a `.hector.yml` in your repo root:

```yaml
schema_version: 2

rules:
  no-debug:
    description: "no DEBUG markers in source"
    engine: script
    scope: ["src/**/*"]
    severity: error
    script: "grep -nE 'DEBUG' {file} && exit 1 || exit 0"
```

Trust it (review the config first — `hector` runs the scripts in it):

```bash
hector trust
```

Run check against a file:

```bash
hector check --file src/foo.rs
```

Exit codes:

- `0` — pass (or warnings only)
- `1` — internal error (config invalid, untrusted)
- `2` — at least one error-severity violation

## Scaffold a starter config

Don't want to write the YAML by hand? Run:

```bash
hector init
```

It detects your stack (Rust, Node, Python) and writes a starter `.hector.yml`. Review it, then run `hector trust`.

## LLM rules

Some rules need an LLM. Add an `llm:` block to your config:

```yaml
llm:
  provider: anthropic
  model: claude-sonnet-4-6
  api_key_env: ANTHROPIC_API_KEY
```

Then add `engine: semantic` or `engine: session` rules. The CLI reads `$ANTHROPIC_API_KEY` from the environment.
