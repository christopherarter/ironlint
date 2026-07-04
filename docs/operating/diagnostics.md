# Diagnostics

When checks misbehave — hooks not firing, an "untrusted config" error, a check that won't run — start with `ironlint doctor`. It's a read-only, minimal static-check command that walks a fixed list of load-time invariants and reports which one is broken and how to fix it:

```bash
ironlint doctor
```

Each row is a check with a status and, when something's wrong, a remediation hint. The command exits `0` when every check is `pass` or `warn`, and `1` when any check `fail`s — so it drops cleanly into CI as a setup step.

For a machine-readable report, add `--format json`. The rest of this page is the contract for that output.

> `ironlint verify` and a fuller `doctor` are planned; today `doctor` runs the static checks below.

## The checks

`doctor` emits these checks, in this order:

| `name` | What it verifies |
|---|---|
| `binary` | The running `ironlint` resolves to a path; reports the version. Always `pass`. |
| `config` | `<dir>/.ironlint.yml` exists. `fail` if missing. |
| `parses` | The config (and every transitive `extends:` ancestor) parses. `fail` on malformed YAML or a rejected legacy config. |
| `check_scripts` | For each check whose `run` is a single-token path beginning with `.ironlint/`, that the path exists and is executable. Inline commands (anything with a space) are skipped. `fail` lists the offending check(s). |
| `trust` | The config and every file under `.ironlint/gates/` is blessed in the out-of-repo trust store. `warn` (not `fail`) when unblessed — `doctor` is read-only, and trust is enforced only at the `check` layer. Remediation: `ironlint trust`. |
| `claude-code` | Harness adapter check: `~/.claude/settings.json` (or project `.claude/settings.local.json`) exists and the registered `PreToolUse` hook artifact is present and unmodified. `fail` if registered but artifact is missing; `warn` if not installed or artifact is modified/outdated. Omitted if the harness is neither present nor registered. |
| `codex` | Harness adapter check: `~/.codex/hooks.json` (or project `.codex/hooks.json`) exists and the registered `PreToolUse` hook artifact is present and unmodified. Same `fail`/`warn` rules as above. This only checks that ironlint's file-system writes are present — it cannot see whether Codex has *reviewed and trusted* the hook, which Codex requires before it will actually run (see the [Codex adapter](../../adapters/codex/README.md)). |
| `pi` | Harness adapter check: the `ironlint.ts` plugin artifact in `.pi/extensions/` (or `~/.pi/agent/extensions/`) is present and unmodified. Same `fail`/`warn` rules as above. |
| `opencode` | Harness adapter check: the `ironlint.ts` plugin artifact in `.opencode/plugins/` is present and unmodified. Same `fail`/`warn` rules as above. |
| `hooks` | Always-present summary row over the adapter checks. `warn` when zero coding-agent hooks are wired (the most common first-run failure mode — the tool's entire effect happens through hooks). Remediation: `ironlint init`. |

Harness checks follow these rules:

- **Registered but artifact missing** → `fail` → exit 1.
- **Not installed / not registered** → omitted entirely (no row emitted).
- **Artifact modified or outdated** → `warn`.
- **Installed and artifact matches** → `pass`.

## Report shape

```json
{
  "ironlint_version": "<x.y.z>",
  "checks": [
    {
      "name": "config",
      "status": "pass",
      "detail": "/work/repo/.ironlint.yml exists",
      "remediation": null
    },
    {
      "name": "claude-code",
      "status": "pass",
      "detail": "installed and registered",
      "remediation": null
    }
  ]
}
```

| Field | Type | Meaning |
|---|---|---|
| `ironlint_version` | string | Version of the running `ironlint` binary. |
| `checks` | array of check objects | One per check, in the order above. Harness rows are included only when that harness is installed or registered. |

Each check object:

| Field | Type | Meaning |
|---|---|---|
| `name` | string | Stable check id. For harness adapter checks, the name is the harness name: `claude-code`, `codex`, `pi`, or `opencode`. |
| `status` | `"pass"` \| `"warn"` \| `"fail"` | Outcome. Any `fail` → exit `1`; otherwise → exit `0`. |
| `detail` | string | One short sentence on what was checked and found. May contain absolute paths or version numbers. |
| `remediation` | string \| null | Actionable hint when `status` is not `pass`; `null` on pass. |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Every check is `pass` or `warn`. |
| `1` | At least one check is `fail`. A registered adapter with a missing artifact drives exit 1. |

These are *distinct* from `ironlint check`'s `0`/`1`/`2`/`3` contract. `doctor` never produces a `Verdict` and never participates in adapter exit-code routing.

## Stability

- The set of `name` values is **additive-only** — new checks land at the end of the list.
- The `status` values (`pass` / `warn` / `fail`) are frozen.
- `detail` and `remediation` strings are human-readable and may change between releases — do not parse them.
- The exit-code rule (`0` for pass-or-warn, `1` for any fail) is frozen.
