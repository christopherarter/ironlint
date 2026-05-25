# Hector — Capability Enforcement

## Status by platform

| Platform | `network: false` | `writes: none` / `cwd-only` |
|----------|------------------|------------------------------|
| Linux    | Per-rule (cloned child sandboxed in `CLONE_NEWNET`; parent untouched) | Best-effort (requires user-namespace privilege; degrades gracefully) |
| macOS    | Best-effort (advisory, logged) | Best-effort (advisory, logged) |
| Windows  | Not supported in 0.1 | Not supported in 0.1 |

## Threat model

Capabilities protect against accidental damage from misconfigured `script:` rules, not against adversarial rule authors. The `trust` gate is the primary defense: rules cannot run until the user reviews and trusts the config.

## Writes policy enforcement (0.1)

The schema accepts `writes: none | cwd_only | tmp | unrestricted` but
**0.1 does not enforce any of them**. All four behave identically:
the spawned process can write anywhere it has POSIX permission to.

Why: enforcement requires CAP_SYS_ADMIN inside a user namespace plus
careful bind-mount remounts; the work is tracked for 0.2. Until then,
treat `writes:` as advisory documentation, not as a control.

If you need write isolation today, run hector inside an OS-level
sandbox (e.g., a container, a fresh user, or `bwrap`).

## Capabilities and runtime safety

`Capabilities::default()` enables network and unrestricted writes — the
maximally-permissive shape. This is intentional: rules that need to
isolate themselves opt in via `capabilities: { network: false, writes:
none }`.

On Linux the network sandbox is **per-rule**: each script subprocess is
spawned via `clone(2)` with `CLONE_NEWNET` applied to the child only.
The parent never `unshare`s, so a `network: false` rule cannot leak its
isolation into a subsequent `network: true` rule that runs in the same
`hector` invocation. This is the B6 fix from the 2026-05-24 audit
(`docs/audits/2026-05-24-check-end-to-end-audit.md#b6`); before it, the
first restrictive rule mutated the parent process's namespaces and
silently broke every later rule.

The capability schema is still not a security boundary against
adversarial rule authors — the trust gate is the primary defense, and
operators concerned about exfiltration from a compromised rule **should
also run hector inside an OS-level sandbox** (container, fresh user,
`bwrap`). The writes policy remains a documented no-op in 0.1.

### Linux fallback for unprivileged hosts

If `clone(2)` returns `EPERM` (typical for unprivileged users without
`CLONE_NEWUSER`), hector falls back to an unsandboxed `std::process`
spawn with a one-time stderr warning. The fallback preserves the P0-8
"never block on missing privilege" guarantee at the cost of running the
rule without isolation. The fallback never mutates the parent process.

### miri exemption

The `clone(2)` call is opaque to miri. Per-rule isolation is verified
empirically by `crates/hector-core/tests/capability_per_child.rs`
(Linux-gated integration tests that compare `/proc/self/ns/net` symlinks
across rules, and exercise a `network: true` rule after a `network:
false` rule to prove the loopback is still visible).

## Roadmap

macOS sandbox profile integration is tracked in `specs/2026-05-11-hector-plan-and-0.1-design.md` §13 (risks). Re-evaluated at 1.0.

## History

- **B6 (2026-05-25):** per-child `clone(2)` for capability isolation. Replaced parent-process `unshare(CLONE_NEWNET)` with per-subprocess `clone(2)` so namespace flags are local to the cloned child. Per-rule capability opt-in now behaves as documented.
