# Rename `.ironlint/gates/` to `.ironlint/scripts/`

## Status

Approved design — ready for implementation plan.

## Goal

Make the policy-script vocabulary match user intuition: **checks** are configured in `.ironlint.yml`, **scripts** live under `.ironlint/scripts/`, and both are covered by `ironlint trust`. The AI can author policy files via Write/Edit, but cannot bless them; the user must manually run `ironlint trust`.

## Background

The current trust surface uses `.ironlint/gates/` for executable check scripts and separately hashes arbitrary in-repo scripts referenced by `run:`/`steps[].run:`. The word "gates" is overloaded: it is the old config key (`gates:`), the bash-gate feature, and the policy directory. The separate "scripts" category is confusing because it sounds like it should be a dedicated directory. This design removes the ambiguity with a hard rename.

## User-visible changes

- `.ironlint/gates/` is retired. Existing projects must move scripts to `.ironlint/scripts/` and re-run `ironlint trust`.
- `ironlint trust` output becomes:
  ```text
  trusted: /path/.ironlint.yml
    config sha256: abcd1234…
    checks: 6
    scripts: 2
      - lint.sh
      - no-todo.sh
  ```
- `ironlint doctor` checks `.ironlint/scripts/*` for existence and executability.
- `ironlint gate-bash` blocks Bash writes to `.ironlint.yml` and `.ironlint/scripts/`.
- `ironlint init` scaffolds the same baseline inline checks; it does not create a scripts dir.

## Architecture

### Trust hash

The trust hash covers:

1. The resolved `.ironlint.yml` bytes.
2. Every regular file under `PROJECT_ROOT/.ironlint/scripts/` (sorted by relative path).

Arbitrary in-repo scripts referenced by `run:`/`steps[].run:` but located outside `.ironlint/scripts/` are **no longer folded into the hash**. A check may still reference them, but changing them does not revoke trust. This is a deliberate simplification: if a script is part of the policy surface, it belongs in `.ironlint/scripts/`.

### Bash-gate policy surface

The bash-gate blocks the agent's Bash tool from:

- running `ironlint trust` (any args), and
- writing to `.ironlint.yml` or any file under `.ironlint/scripts/`.

### Adapter hooks

All adapters short-circuit Write/Edit on files inside `PROJECT_ROOT/.ironlint/scripts/`, exactly as they already short-circuit `.ironlint.yml`. This lets the AI author policy scripts while the trust gate remains enforced for ordinary files.

The short-circuit check must be path-based, not basename-based, to avoid matching files like `src/.ironlint/scripts/foo.sh` that are not the project policy surface.

### BlessedSummary

`ironlint_core::trust::BlessedSummary` keeps the fields it needs to render the CLI output:

- `config_path`
- `config_hash`
- `checks: usize` — number of resolved checks
- `scripts: Vec<String>` — relative paths under `.ironlint/scripts/`

The old `gates` and `scripts` fields are consolidated into a single `scripts` concept.

## Security model

- The AI can propose policy changes via Write/Edit (visible to the user).
- The AI cannot run `ironlint trust` via Bash (bash-gate).
- After a policy change, `ironlint check` on ordinary files fails with exit 4 until the user manually runs `ironlint trust`.
- Bash remains blocked from writing policy files directly.

## Files touched

- `crates/ironlint-core/src/trust.rs` — rename gate dir to `.ironlint/scripts/`, drop referenced-scripts fold, update `BlessedSummary`.
- `crates/ironlint-cli/src/commands/trust.rs` — print `checks:` and `scripts:`.
- `crates/ironlint-cli/src/commands/doctor.rs` — check `.ironlint/scripts/`.
- `crates/ironlint-cli/src/commands/gate_bash.rs` — update policy surface path.
- `crates/ironlint-cli/src/cli.rs` — help text.
- `adapters/claude-code/hooks/hook.sh` — short-circuit `.ironlint/scripts/`.
- `adapters/codex/hooks/hook.sh` — short-circuit `.ironlint/scripts/`.
- `adapters/pi/src/index.ts` — add `.ironlint/scripts/` to `isPolicyFile`.
- `adapters/opencode/src/index.ts` — add `.ironlint/scripts/` to `isPolicyFile`.
- Tests and existing docs/specs referencing `.ironlint/gates/`.

## Testing

- Update `trust.rs` tests for `.ironlint/scripts/` paths and `BlessedSummary` shape.
- Update `doctor.rs` tests for `.ironlint/scripts/` paths.
- Update `gate_bash.rs` tests for `.ironlint/scripts/` policy surface.
- Add test: adapter hook allows writing `.ironlint/scripts/foo.sh` while untrusted.
- Add test: `ironlint trust` output shows `checks: N` and `scripts: N`.
- Add test: bash-gate blocks `cp x .ironlint/scripts/foo.sh`.

## Open questions (none)

Design approved by principal engineer.
