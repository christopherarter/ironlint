# IronLint — Codex adapter

`PreToolUse` hook integration for OpenAI's Codex CLI (0.141+). Runs `ironlint
check` on every `apply_patch` file edit **before** the edit lands on disk,
checking the proposed content against your project's `.ironlint.yml` policy.

> **Timing:** this adapter fires on **PreToolUse**, so ironlint sees the
> proposed content — parsed out of the `apply_patch` envelope in
> `tool_input.command` and spliced against the on-disk file for an update —
> before Codex writes it. This mirrors the claude-code and pi adapters, which
> also gate pre-write.

> **Codex's block contract is not the exit-code contract.** A `PreToolUse`
> hook blocks by printing a `permissionDecision:"deny"` JSON object on
> **stdout** and exiting **0** — the exit code alone never blocks, and
> malformed or garbage stdout on a would-be block **fails open** (the edit
> lands). `hooks/hook.sh` is written defensively around this: every block path
> builds the deny JSON with `jq` and falls back to a static, guaranteed-valid
> deny payload if `jq` itself fails, so a decided block is never silently
> dropped. See [Guardrail, not a hard boundary](#guardrail-not-a-hard-boundary)
> below for the scope this hook can and can't cover.

## Install

```bash
ironlint init --harness codex
```

This auto-detects Codex (`~/.codex` present) and writes
`<project>/.codex/hooks.json` (or `~/.codex/hooks.json` with `--global`) with
a `PreToolUse` entry matching `apply_patch|Edit|Write` — Codex aliases all
three to `apply_patch` for file edits, and the hook itself only acts when
`tool_name == "apply_patch"`. The adapter artifact is written atomically to
`~/.config/ironlint/adapters/codex/hook.sh`, with a `.ironlint-adapter.json`
sidecar (per-file sha256 + version) alongside it. A backup of any prior
`hooks.json` is saved as `hooks.json.bak` on the first write; re-runs are
idempotent (unchanged → "already present", changed artifact → "updated").

**ironlint writes `hooks.json`, never `config.toml`.** Codex also supports an
inline `[hooks]` table in `config.toml`; ironlint always targets the
dedicated `hooks.json` file so its writes never collide with anything you
hand-author in `config.toml`.

Verify the install:

```bash
ironlint doctor
```

To remove the hook:

```bash
ironlint init --uninstall --harness codex
```

This removes the hook entry, the materialized artifact, and the sidecar.
Your `.ironlint.yml` and trust store are untouched.

## Required manual step: review + trust the hook in Codex

Writing `hooks.json` is not sufficient on its own. Codex treats any
non-managed hook — anything it did not ship itself — as untrusted until a
human reviews and approves it, so a `hooks.json` entry ironlint writes will
**not run** until you complete that review inside Codex. After `ironlint
init --harness codex` (or the bare `ironlint init` that detects it), open
Codex and step through its hook trust prompt before relying on the gate.

`ironlint doctor` reports the hook as installed and registered as soon as the
file is on disk — that's a filesystem fact. Whether Codex has trusted it and
will actually run it is a Codex-side decision doctor cannot observe, so a
`pass` from `ironlint doctor` does not by itself mean edits are being gated.

## Timeout budget

This hook's own `timeout` is registered at **120s** — 4× IronLint's default
per-check wall-clock cap (`execution.timeout_secs`, 30s by default; see
[Timeout budget](../../docs/adapters/README.md#timeout-budget) for the general
rule). Checks dispatch sequentially, so a file matching several slow checks
can burn multiples of that cap before `ironlint check` reports a verdict. If
this hook's timeout were lower than that worst case, Codex would kill the hook
process first and the edit would land **ungated**, with no signal to anyone —
a silent bypass, not the fail-open behavior on exit `3` described above.

If you raise `execution.timeout_secs` above the default, re-run `ironlint init
--harness codex` so the regenerated `hooks.json` entry keeps enough headroom
(or hand-edit `timeout` in `hooks.json` to at least `4 × timeout_secs`).

## Guardrail, not a hard boundary

Codex's own documentation describes `PreToolUse` as something a model "can
often" route around through another supported tool path — shell interception
via `unified_exec` is incomplete, so a determined model can reach the
filesystem through a path this hook never sees. That's a property of Codex's
hook design, not a gap in this adapter: the gate covers everything Codex
routes through `apply_patch` (what `Edit`/`Write`-shaped file edits use), but
it is a weaker enforcement boundary than the claude-code adapter gives you.
Treat it as a strong guardrail, not a guarantee.

## Manual fallback

Use these steps if the `ironlint` binary is not available:

1. Install the `ironlint` binary (`cargo install --git https://github.com/christopherarter/ironlint ironlint-cli`, or use a release binary).
2. Copy `hooks/hook.sh` from this directory into place and register it in
   Codex's `hooks.json` under `hooks.PreToolUse`, matcher
   `apply_patch|Edit|Write`, `command` pointed at the script's absolute path
   plus the argument `pre-tool-use`, `timeout` `120` (seconds — see
   [Timeout budget](#timeout-budget) above for why).
3. Run `ironlint init` in a project to scaffold `.ironlint.yml`.
4. Review the config and run `ironlint trust` to fingerprint it.
5. In Codex, review and trust the newly registered hook (see above) — it will
   not fire until you do.
6. Edit a file — the `PreToolUse` hook checks the proposed edit against the
   policy *before* it lands; a violating edit is blocked and never written.

## Requirements

- `ironlint` binary on PATH.
- `bash`, `jq`, `python3` on PATH (required at hook runtime — `python3`
  parses the `apply_patch` envelope and synthesizes each touched file's
  post-edit content; `jq` builds the deny JSON and parses the incoming event).

## How the hook resolves

`hooks.json` points `command` at the absolute path of the materialized
`hook.sh` under `~/.config/ironlint/adapters/codex/` — no `${VAR}`
indirection, unlike the claude-code plugin layout. If Codex reports the hook
command as missing, re-run `ironlint init --harness codex` to re-materialize
it, or check that `~/.config/ironlint/adapters/codex/hook.sh` exists and is
executable.
