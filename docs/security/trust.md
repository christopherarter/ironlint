# The trust store

A `.ironlint.yml` runs arbitrary shell through its checks' `run:` commands. So IronLint refuses to run a config you haven't vouched for: you review the config and its check scripts, bless them once, and IronLint verifies that blessing before every `check`. This is the primary defense against a malicious or tampered config.

The blessing lives **outside the repo**, so a config can't vouch for itself — a pulled, generated, or freshly-edited config is untrusted until *you*, on *this* machine, bless it.

## Blessing a config

After writing or editing your checks, review them, then bless:

```bash
ironlint trust
```

This computes a SHA-256 over the config and its check scripts and records it in the trust store at `~/.config/ironlint/trust.json` (or `$XDG_CONFIG_HOME/ironlint/trust.json`), keyed by the config's absolute path:

```json
{
  "version": 1,
  "entries": {
    "/home/you/project/.ironlint.yml": {
      "hash": "sha256:8798ad5a0ab624c9a5d56b87372cdaf1fdd3ccc5339fe2573b82b26be28b9f36",
      "blessed_at": "2026-06-24T18:03:11+00:00"
    }
  }
}
```

To bless a config other than `.ironlint.yml`:

```bash
ironlint trust --config shared/base.yml
```

## The agent can't bless its own config

The Bash tool is gated too: an agent running `ironlint trust` (or writing to
`.ironlint.yml` / a gate script through Bash redirections, `tee`, `sed -i`,
`cp`/`mv` onto the policy surface) is denied. The Write/Edit path to those
files stays open — it is already gated — so the change closes the *ungated*
Bash escape without removing the legitimate edit path. The deny decision is
shared across every adapter via `ironlint gate-bash`, and it fires even in a
project with no `.ironlint.yml` (exactly when the agent is most motivated to
self-trust). See
`docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`
for the threat model and the documented known gap (variable-substitution
indirection — adversarial tier, out of scope).

## How verification works

Before loading the engine or running any check, `ironlint check` recomputes the hash and compares it to the blessed entry for that config's path. On a missing or mismatched entry it stops with a config error (exit `1`) and a hint to re-bless — no check runs:

```
config/gates not trusted — review and run `ironlint trust`
```

Only `check` enforces trust. The read-only commands — `validate`, `explain`, `show-resolved-config`, `doctor` — never do, so you can inspect an untrusted config without blessing it first.

Any change to a covered file invalidates the hash. That's the point: a config, or a check script, that's been edited — by you, a teammate, or anything else — since you last reviewed it won't run until you look at the change and re-bless.

## Re-blessing after a change

Re-run `ironlint trust` whenever you change anything it covers (see [What trust covers](#what-trust-covers)):

- the config file, or any file it `extends:`
- any script under a covered `.ironlint/gates/`

The workflow is: edit checks → review → `ironlint trust` → commit. If you pull a change to `.ironlint.yml` (or a base it extends) from a teammate, review their diff before blessing it on your machine.

The trust store lives outside the repo, so it isn't committed and isn't shared — every machine blesses for itself. Moving the project to a new path, or upgrading IronLint across a version that changes the hash algorithm, also needs a one-time re-bless; the mismatch error from `check` will say so.

## What trust covers

The blessed hash folds, in a fixed, deterministic order:

1. **Every config file in the `extends:` closure** — the config you check, plus every file it transitively extends.
2. **Every file under each of their `.ironlint/gates/` directories.**

So with `extends:` you bless the **root** config you run `check` against, and a single `ironlint trust` covers the whole chain. Editing a parent — or a parent's check script — invalidates the root's hash and forces a re-review. (You only bless a parent separately if you also `check` it directly as a root of its own.)

### What it doesn't cover

- **Check scripts outside `.ironlint/gates/`.** A check whose `run:` shells out to a file elsewhere in the repo — `run: "bash scripts/lint.sh"` or `run: "python tools/scan.py"` — is covered only for the `run:` *string* (which lives in the config). The contents of `scripts/lint.sh` are **not** hashed, so editing that file can neuter the check without invalidating trust. Keep check logic under `.ironlint/gates/` (e.g. `run: ".ironlint/gates/lint.sh"`) to bring it inside the boundary. This is the same threat class as a tampered config, reached through a file the hash doesn't reach.
- **Interpreters and tools on `$PATH`.** Trust vouches for your check scripts, not for the `python`, `grep`, or `node` they invoke.
- **Writes during the run.** The hash is computed when `check` starts; a write between that point and a check actually executing isn't caught. This TOCTOU window is a known limitation of the direnv-style model — there's no file locking in 0.3.

## Trust is not a sandbox

The trust store answers "have I reviewed what runs?", not "what can it do once it runs?". IronLint 0.3 does **not** sandbox check commands — the per-check [timeout](../operating/running-checks.md) is the only execution rail, and a blessed check runs with your full user privileges.

If you need real isolation from a config you don't fully trust, run IronLint inside an OS-level sandbox — a container, a fresh user, or `bwrap` — in addition to blessing it.

## Checks run with a scrubbed environment

A blessed check still doesn't see your whole shell environment. Before spawning a check, IronLint clears the child process's environment and rebuilds it from an explicit allowlist: `$PATH`, `$HOME`, locale (`$LANG` and any `$LC_*`), `$TZ`, `$TMPDIR`, plus the [`$IRONLINT_*` ABI vars](../writing-checks/README.md#the-abi-what-every-check-receives). Everything else in the parent process's environment — `$ANTHROPIC_API_KEY`, `$GITHUB_TOKEN`, `$AWS_*`, any other secret or token the agent process holds — is **not** passed through, even to a fully blessed check. This bounds what a compromised or careless check script can read, on top of (not instead of) trust review.

## See also

- [Sharing config with `extends:`](../configuring/inheritance.md) — blessing a config that inherits
- [Running checks](../operating/running-checks.md) — where trust sits in the `check` flow
- [Getting started](../getting-started.md) — trust in the first-run workflow
