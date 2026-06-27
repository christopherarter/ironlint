# `hector init` — rtk-style harness onboarding

**Status:** design, approved direction (2026-06-27)
**Builds on:** the gates model in `specs/2026-06-15-hector-gates-redesign-design.md`
**Relates to:** the adapter ABI / `hector hook <harness>` work (later "Plan 4"); this spec is the *onboarding* layer, not the ABI rewrite.
**Breaking:** mild. `hector init` stops erroring when `.hector.yml` already exists; the per-adapter shell installers (e.g. `adapters/reasonix/install.sh`) are superseded.

## 1. Thesis

Hector already ships four working adapters (`adapters/{claude-code,reasonix,pi,opencode}`) and a trust store, but onboarding is bespoke per harness: a `jq`-dependent `install.sh` for reasonix, copy-paste READMEs for the rest. rtk's onboarding feels good because **one self-contained binary command** wires the tool into whichever harness you point it at — embedded templates, atomic settings patches, idempotency, integrity sidecars, dry-run, uninstall, and per-harness "now restart X" hints.

We replicate that, folded into the command Hector already has: `hector init`. After this change, a single `hector init` scaffolds the config *and* offers to wire the hook into every coding agent it detects on the machine — no `jq`, no loose scripts to locate, no repo checkout required at runtime.

Scope is deliberately the **four hook-capable harnesses only**. Hector is a PostToolUse *gate* that must execute to block (exit `2`); a rules-markdown file that merely *advises* an LLM to "please run hector" can't enforce, so rules-only harnesses (cursor/windsurf/cline/…) are explicitly out. We onboard only where the gate genuinely gates.

## 2. What rtk does (the model we copy)

`rtk init --agent <harness>` (`src/hooks/init.rs`): resolves the harness home, atomically writes its hook artifact (binary-command hook patched into a JSON settings file, or a TS/Python plugin), merges idempotently into existing config, writes a SHA-256 sidecar for tamper/drift detection, supports `--dry-run`/`--uninstall`, and prints a structured summary with a per-harness next-step hint. The hook templates are `include_str!`'d into the binary, so the install needs nothing on disk but `rtk` itself.

We keep the shape, change three things to fit Hector:

1. **One command, not a new verb.** Onboarding lives inside `hector init`, matching rtk's single-command feel while reusing the command users already reach for.
2. **Gate semantics, not rewrite semantics.** The patched hook runs `hector check` and honors the block contract (exit `2` → deny); there is no command rewriting and no advisory-rules fallback.
3. **Detect-then-confirm**, not manual `--agent`. Bare `hector init` auto-detects installed harnesses and confirms before writing — strictly better UX than rtk's mandatory flag.

## 3. Command surface

```
hector init [--dir DIR]
            [--harness <claude-code|reasonix|pi|opencode|all>]   # repeatable; omit = detect
            [--global]        # patch user-level settings instead of project-local
            [--yes]           # skip the confirm prompt
            [--no-hook]       # config only (today's behavior)
            [--hook-only]     # skip config scaffolding, only wire hooks
            [--uninstall]     # remove hector hooks + materialized artifacts
            [--dry-run]       # print intended changes, write nothing
```

Flag interactions:

- `--no-hook` reproduces today's exact behavior (scaffold config, install nothing).
- `--hook-only` skips the config phase entirely (for repos that already have `.hector.yml`).
- `--no-hook --hook-only` together is a usage error.
- `--harness` is repeatable (`--harness claude-code --harness pi`); `--harness all` selects all four; any `--harness` suppresses the confirm prompt for the named set.
- `--uninstall` ignores `--no-hook`/`--hook-only` and operates only on hooks.

## 4. Run flow

1. **Config phase.** If `.hector.yml` is absent: scaffold + `trust::bless` it (today's logic, unchanged). If present: print `config: .hector.yml already present (skipped)` and continue — *no longer a hard error*. Skipped entirely under `--hook-only`.
2. **Detect phase.** For each harness, check whether it is installed (§6 detection table). Produce `{harness, detected: bool, target_path}`.
3. **Select phase.**
   - `--harness <name>`: select exactly those. Error if a named harness's home is absent *and* `--global` is not set (can't install into something that isn't there).
   - `--harness all`: select all four regardless of detection.
   - Bare: list detected harnesses and prompt `Install hector hooks into these? [Y/n]`. `--yes` auto-accepts. **No TTY and no `--yes`/`--harness`** → skip the hook phase, print a hint (`run \`hector init --harness all\` to wire hooks`). This keeps CI runs of `hector init` side-effect-free.
4. **Install phase** (per selected harness, §7).
5. **Summary.** One line per harness (`installed` / `already present` / `updated` / `failed: <reason>`) plus the harness's restart/test hint.

## 5. Architecture

A `Harness` registry in `hector-core` (new `adapter` module) is the single extension point. Each entry is data + a small behavior set:

```
struct Harness {
    name: &str,                       // "claude-code"
    detect: fn(&Env) -> Option<PathBuf>,   // Some(home) if installed
    settings_path: fn(&Env, Scope) -> PathBuf,
    artifact: Artifact,               // embedded files + materialize target
    mechanism: Mechanism,             // JsonHook { event, matcher } | Plugin { ... }
    restart_hint: &str,
}
```

- `Artifact` carries the `include_str!`'d adapter file bytes and the relative path they materialize to. Source of truth stays under `adapters/<harness>/`; the binary embeds them at compile time. A build-time test asserts the embedded set matches the on-disk adapter tree so the two never silently diverge.
- `Mechanism` splits the two install shapes:
  - **`JsonHook`** (claude-code, reasonix): patch a JSON settings file's hooks array. claude-code → `PostToolUse` matcher `Edit|Write`; reasonix → `PreToolUse` matcher `^(write_file|edit_file|multi_edit)$`. The jq logic in the current `reasonix/install.sh` is ported to `serde_json` (insert-if-absent, strip stale hector entries by command-path substring).
  - **`Plugin`** (pi, opencode): materialize a `.ts` plugin into the harness's plugin directory; patch a manifest only if that harness requires explicit registration (per the existing adapter README — the plan phase pins the exact paths from `adapters/{pi,opencode}/README.md`).

`hector-cli`'s `init` command stays a thin adapter: it calls into the core `adapter` module for detect / install / uninstall / status and owns only the prompt, the flag parsing, and the printed summary.

**Trust boundary.** Per the repo's standing rule, `HectorEngine::load` stays pure and the trust *store* is untouched. Hook integrity (§7) is a separate, lighter concern: a content hash of the materialized artifact, not a config-trust decision.

## 6. Detection & install targets

| Harness | Detected when present | Default (project-local) target | `--global` target | Artifact materialized to |
|---|---|---|---|---|
| `claude-code` | `~/.claude/` exists | `<project>/.claude/settings.json` | `~/.claude/settings.json` | `$XDG_CONFIG_HOME/hector/adapters/claude-code/hook.sh` (+ `synthesize_diff.sh`) |
| `reasonix` | `~/.reasonix/` exists | `~/.reasonix/settings.json` (no project scope) | same | `$XDG_CONFIG_HOME/hector/adapters/reasonix/hook.sh` |
| `pi` | pi config/plugin dir exists | project plugin dir if supported, else user | user plugin dir | harness plugin dir (`rtk.ts` analog) |
| `opencode` | opencode config dir exists | project plugin dir if supported, else user | user plugin dir | harness plugin dir |

`$XDG_CONFIG_HOME` defaults to `~/.config`, so shell-hook artifacts live under `~/.config/hector/adapters/<harness>/` — alongside the existing trust store at `~/.config/hector/trust.json`. Plugin artifacts must live where the harness's runtime loads them, so for `pi`/`opencode` the materialize target *is* the harness plugin dir, with the `.sha256` sidecar beside it.

**Scope default is project-local** where the harness supports it (claude-code definitely; pi/opencode via project plugin dirs), so `hector init` in a repo wires that repo. reasonix has only a user-global settings file, so it is global regardless. `--global` forces user-level everywhere it is meaningful. The patched hook resolves `.hector.yml` from cwd at runtime and fails open when absent, so even a global install is inert outside hector projects.

## 7. Install mechanics

For each selected harness:

1. **Materialize** the embedded artifact bytes to the target path (creating parent dirs), prepending a `# hector-adapter-version: N` marker to shell hooks.
2. **Sidecar** — write `<artifact>.sha256` with the artifact's content hash. Not a security boundary; it lets `doctor` detect post-install drift/tamper and lets re-install detect "needs update".
3. **Patch settings** — read the JSON/plugin manifest, edit in memory, write a tempfile in the *same directory*, fsync, atomic `rename`. The pre-existing file is copied to `<settings>.bak-<timestamp>` first. Patching is idempotent: an existing hector entry → leave it, report `already present`; an entry whose materialized artifact hash differs from the binary's embedded copy → rewrite artifact, report `updated`.

`--dry-run` runs phases 1–3 in describe-only mode: it prints every path it would write and every settings key it would add, and writes nothing (no artifact, no sidecar, no backup).

## 8. `hector doctor` integration

`doctor` (today minimal, read-only, exit 1 on failure) gains an **adapters** section. Per harness:

- installed? (artifact present at materialize target)
- registered? (settings file contains the hector hook entry)
- intact? (artifact hash matches its `.sha256` sidecar)
- current? (artifact matches the binary's embedded version — else "outdated: re-run `hector init`")

A drifted or missing entry is a *warning*, not a failure, unless the artifact is registered-but-missing (a broken hook), which is a failure. Output respects the existing `--format human|json` contract.

## 9. Uninstall

`hector init --uninstall [--harness X]`:

- removes the hector hook entry from each selected harness's settings (atomic write + `.bak` backup);
- deletes the materialized artifact and its `.sha256` sidecar;
- leaves `.hector.yml` and the trust store untouched (config ≠ hook);
- honors `--dry-run`.

With no `--harness`, uninstall targets every harness it finds a hector artifact/entry for.

## 10. Error handling

- Per-harness install failures (unwritable settings, malformed existing JSON, missing harness home under explicit `--harness`) are caught, reported in the summary as `failed: <reason>`, and do **not** abort the other harnesses.
- Exit code: `0` if at least one selected harness succeeded (or there was nothing to do); non-zero only if **every** selected harness failed, or on a usage error (`--no-hook --hook-only`).
- The hooks themselves already fail open (exit `0` when no `.hector.yml`), so onboarding never makes a non-hector project worse.

## 11. Testing

- **Unit (`hector-core::adapter`)**: detection against a temp `HOME`/XDG env; `serde_json` patch/unpatch idempotency (insert, re-insert no-op, stale-entry strip); sidecar hash round-trip; version-marker upgrade path; embedded-artifacts-match-on-disk-tree build test.
- **Integration (`hector-cli`, `assert_cmd`)**: `init --harness claude-code --yes` in a temp project asserts the `PostToolUse` entry points at the materialized hook and that artifact + sidecar exist; re-run reports `already present`; `--uninstall` removes entry + artifact; `--dry-run` writes nothing; `--no-hook` reproduces config-only; an existing `.hector.yml` no longer errors.
- Gates: each touched file under `crates/*/src/` clears the ≥80% region-coverage bar (`scripts/ci-coverage.sh`); per-function cognitive complexity ≤15 (patch dispatch decomposed per mechanism, not one mega-`match`).

## 12. Out of scope (YAGNI)

- No harnesses beyond the four hook-capable ones; no advisory rules-markdown path.
- No pure-binary hook port. The materialized artifact is still the existing shell/TS adapter. Folding hook logic into a `hector hook <harness>` subcommand (so the hook *is* the binary, à la `rtk rewrite`) is the natural next step but belongs to the adapter-ABI work, not here.
- No background update daemon and no 24-hour outdated-hook warning cooldown (rtk has both). `hector doctor` surfaces staleness on demand instead.
- No changes to the trust store or `HectorEngine::load`.

## 13. Migration notes

- `adapters/reasonix/install.sh` and the manual-install sections of each adapter README are superseded by `hector init`; update them to point at the new command (keep a one-paragraph "manual fallback" only if a harness can't be auto-detected).
- The claude-code adapter is currently distributed as a plugin (`.claude-plugin/plugin.json`). `hector init` installs via a direct settings patch instead, which does not depend on the plugin marketplace; the plugin packaging can remain for users who prefer it.
