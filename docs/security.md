# Hector — Capability Enforcement

## Status by platform

| Platform | `network: false` | `writes: none` / `cwd-only` |
|----------|------------------|------------------------------|
| Linux    | Strict (CLONE_NEWNET namespace) | Best-effort (requires user-namespace privilege; degrades gracefully) |
| macOS    | Best-effort (advisory, logged) | Best-effort (advisory, logged) |
| Windows  | Not supported in 0.1 | Not supported in 0.1 |

## Threat model

Capabilities protect against accidental damage from misconfigured `script:` rules, not against adversarial rule authors. The `trust` gate is the primary defense: rules cannot run until the user reviews and trusts the config.

## Roadmap

macOS sandbox profile integration is tracked in `specs/2026-05-11-hector-plan-and-0.1-design.md` §13 (risks). Re-evaluated at 1.0.
