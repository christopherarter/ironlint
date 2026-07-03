# Codex `apply_patch` PreToolUse payload fixtures

Ground-truth captures of what Codex sends a **PreToolUse** hook on stdin for an
`apply_patch` file edit. Used to validate the codex adapter's envelope parser
(`adapters/codex/hooks/hook.sh`).

## Provenance

Captured **2026-07-03** from `codex-cli 0.141.0` (model `gpt-5.5`) via a single
interactive run in a scratch project carrying a project-local logging hook:

```json
{ "hooks": { "PreToolUse": [ { "matcher": "apply_patch|Edit|Write",
  "hooks": [ { "type": "command", "command": "jq -c . >> <capture>.ndjson; exit 0" } ] } ] } }
```

Task driven: create `foo.py` = `print('hi')`, then update it to `print('bye')`.
`apply_patch` fired twice → the two fixtures here.

## Sanitization

Only `cwd` and the volatile per-session ids are altered; the **`tool_input.command`
envelope is byte-verbatim from the capture**:

- `cwd` → `__CWD__` (tests substitute the temp project path)
- `session_id` / `turn_id` / `tool_use_id` → placeholder values
- `transcript_path` → `null`

## Contract facts these lock

- `tool_name` is **`apply_patch`** (even though the matcher may alias `Edit`/`Write`).
- The patch lives in **`tool_input.command`** as a **bare envelope** — `*** Begin Patch`
  … `*** End Patch`, **not** heredoc-wrapped, and `*** End Patch` carries **no trailing
  newline**.
- **Add File** section: added lines are `+`-prefixed.
- **Update File** section: an `@@` hunk header (which may be **empty** — just `@@`),
  then removed (`-`) / added (`+`) / context (space-prefixed) lines.

If a future Codex version changes this shape, re-capture and update both the fixtures
and the parser; the parser fails **closed** (blocks the edit) on any shape it can't read,
so drift surfaces as a visible block, never a silent bypass.
