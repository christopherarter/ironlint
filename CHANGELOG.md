# Changelog

Notable changes to IronLint, newest first. In-flight work lives in `plans/`.

## [Unreleased]

## [0.10.0] — 2026-07-09 — Rename `.ironlint/gates/` → `.ironlint/scripts/`

The policy-script vocabulary now matches user intuition: **checks** are
configured in `.ironlint.yml`, **scripts** live under `.ironlint/scripts/`,
and both are covered by `ironlint trust`. The word "gates" was overloaded —
the old `gates:` config key, the bash-gate feature, *and* the policy
directory — so the directory is renamed and the overlapping "referenced
scripts anywhere in the repo" category is removed. This is a **breaking
change**: existing projects must move scripts to `.ironlint/scripts/` and
re-run `ironlint trust`.

### Breaking

- **`.ironlint/gates/` is retired.** Move scripts to `.ironlint/scripts/` and
  re-run `ironlint trust`. There is no migration path and no backward-compat
  shim — no install base exists.
- **Referenced scripts outside `.ironlint/scripts/` are no longer hashed.** A
  script referenced by a check's `run:`/`steps[].run` but located elsewhere in
  the repo is no longer folded into the trust hash, so editing it does not
  revoke trust. This is a deliberate simplification: if a script is part of
  the policy surface, it belongs in `.ironlint/scripts/`. It also makes the
  trust hash surface equal to the bash-gate enforcement surface (both =
  `.ironlint/scripts/`) — the bash-gate cannot defend an arbitrary out-of-dir
  script from agent tampering, so the hash must not pretend to cover it.

### Changed

- **`ironlint trust` summary** now renders `checks: N` and `scripts: N` (with
  per-script names), replacing the old `gates:` list. The scripts block is
  always shown, even at zero.
- **`ironlint doctor`** checks scripts under `.ironlint/scripts/` for
  existence and executability (was `.ironlint/gates/`).
- **`ironlint gate-bash`** blocks Bash writes to `.ironlint/scripts/` (was
  `.ironlint/gates/`); the legacy path is now allowed.
- **Adapter hooks** (claude-code, codex, pi, opencode) short-circuit edits to
  `.ironlint/scripts/` path-anchored to the project root — matching
  `src/.ironlint/scripts/` is correctly *gated*, not short-circuited (a
  basename match would be a bug).

### Fixed

- Stale "gates" terminology swept across docs, adapter READMEs, and test
  files. A pinning test (`out_of_dir_referenced_script_is_absent_from_summary`)
  now guards the dropped referenced-scripts fold against silent re-addition.

## [0.9.2] — 2026-07-07 — Bash gate: close `sh -c` and bare `VAR=val` bypasses

Two adjacent bash-gate bypasses found during v0.9.1 release review are now
closed. Both let a lazy non-reasoning model run `ironlint trust` through its
Bash tool despite the matcher. A code review of the v0.9.2 fixes surfaced a
third symmetric form — `dash -c` (and `ash`/`zsh`/`ksh -c`) — which is the
same `-c <command-string>` shape as `sh -c`; the descent now recognizes all
six common shells rather than just `sh`/`bash`.

### Fixed

- **`sh -c 'ironlint trust'` / `bash -c "ironlint trust"`** (and `dash`/`ash`/
  `zsh`/`ksh -c`) — the matcher's `normalize` step strips quotes from the whole
  string, so the gate was analyzing a *different* command than the shell
  executes (it saw `sh -c ironlint trust`, where sh runs only `ironlint` and
  `trust` becomes `$0`). `strip_wrappers` now recognizes the common
  `-c <command-string>` shells (`sh`, `bash`, `dash` — which IS `/bin/sh` on
  Debian/Ubuntu, `ash` — the BusyBox sh in Alpine/containers, `zsh`, `ksh`)
  and, when followed by `-c`, descends into the command-string argument tokens
  and re-checks them (mirroring how `eval`/`exec` unwrap to their argument).
  Without `-c` (`sh script.sh`) the wrapper does not descend — that's the
  documented script-file indirection gap (adversarial tier, out of scope).
  **HIGH realism for a lazy model.**
- **Bare `VAR=val ironlint trust`** (env-var prefix without `env`) — the
  `env VAR=val ironlint trust` form was already caught, but the semantically
  equivalent bare prefix (`IRONLINT_ROOT=/x ironlint trust`) was not. The
  wrapper fall-through now recognizes a leading `VAR=val` assignment via a new
  shared `is_assignment` helper (a strict shell-identifier check — letters/
  digits/underscore, not digit-leading — so a leading `--config=x.yml` flag is
  NOT over-skipped) and strips it one token at a time, so multiple leading
  assignments (`FOO=bar BAZ=qux ironlint trust`) all strip. **MEDIUM-HIGH
  realism.**
- Added sibling-token regression pins for `and`/newline/comma forms
  (`ironlint check and ironlint trust`, etc.) — caught today by the
  every-binary scan but previously unpinned. Added `is_assignment` predicate
  pins (underscore-leading blocks; digit-leading allows) that kill two new
  mutation survivors.

### Changed

- `skip_assignments` now uses the strict `is_assignment` helper instead of a
  bare `.contains('=')` — the stricter check is consistent with the new
  bare-prefix arm and does not regress the existing `env VAR=val` tests.

### Known gaps

- **`VAR+=val ironlint trust`** (append-assignment prefix) — **not blocked in
  0.9.2.** `+=` is a valid bash append-assignment exported to the command's
  env, semantically identical to `VAR=val` for running `ironlint trust`, but
  `is_assignment`'s strict shell-identifier check rejects the `+` in the name
  part. **LOWER realism** — `+=` is a specific append idiom, not the obvious
  set form a lazy model reaches for first. Documented as a conscious decision
  rather than an accident; expanding the assignment grammar is scope creep for
  a marginal form. See
  `docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`.

## [0.9.1] — 2026-07-07 — Bash gate: self-trust prevention

Every adapter now gates the agent's Bash tool, closing the escape hatch where an
agent could free itself from ironlint by running `ironlint trust` or writing the
policy surface (`.ironlint.yml`, `.ironlint/gates/`) through Bash redirections.

### Added

- **`ironlint gate-bash`** — a pure-Rust Bash-command classifier (new
  `ironlint-bash-gate` crate). It is not a `check` and not trust-gated; it runs
  even with no `.ironlint.yml` present — exactly when an agent is most motivated
  to self-trust. Exit contract: `0` = allow (empty stdout), `2` = block (reason
  on stdout); any other exit (spawn failure, signal) is treated as fail-closed
  by the adapters.
- **Bash branches in all four adapters** (claude-code, codex, pi, opencode).
  Each adapter's Bash branch shells out to `ironlint gate-bash` with the command
  on stdin, *before* the config-existence check, so the gate fires in
  config-less projects too.
- **Blocked commands** (the matcher denies these):
  - `ironlint trust` — including de-obfuscated forms: prefix wrappers
    (`nohup`, `env`, `exec`, `eval`, `timeout`), full/relative paths
    (`/usr/local/bin/ironlint trust`, `./ironlint trust`), global flags before
    the subcommand (`ironlint -v trust`), chained commands (`ironlint check ||
    ironlint trust`, `&&`/`;`/`|`), `or`-confusion (`ironlint check or
    ironlint trust` — `or` is not a shell operator so sh runs `ironlint trust`
    anyway; the matcher checks every `ironlint` binary in the segment, catching
    the second invocation), and subshell/brace grouping
    (`(ironlint trust)`, `{ ironlint trust; }`).
  - **Bash writes to the policy surface** — redirects (`echo x>.ironlint.yml`,
    `>>`, the `>|` clobber), `tee`, `sed -i`, `ed`, `cp`/`mv`/`install`/`rsync`
    onto `.ironlint.yml` or `.ironlint/gates/`, `dd of=`, and `sponge`. The
    Write/Edit path to those files stays open (it is already gated); this closes
    the *ungated* Bash escape.

### Known gaps

- **Variable-substitution indirection** — `iron$(echo lint) trust`,
  `IRON=ironlint; $IRON trust`, `base64 -d | sh`, or invoking through a script
  file (`bash scripts/x.sh` where `x.sh` runs `ironlint trust`) is **not
  blocked**. Catching it requires real shell evaluation, which crosses into the
  adversarial tier this gate explicitly scopes out. The threat tier is "lazy
  non-reasoning models"; the honest contract is pinning both what it catches and
  what it doesn't. See
  `docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`.
- **`sh -c 'ironlint trust'` / `bash -c 'ironlint trust'`** — **not blocked in
  0.9.1.** The matcher's `normalize` step strips quotes from the whole string,
  so the gate analyzes a *different* command than the shell executes (it sees
  `sh -c ironlint trust`, where sh runs only `ironlint`, instead of the quoted
  form where sh runs `ironlint trust`). The literal string `ironlint trust` is
  present in the command but the wrapper-descent into `sh -c`/`bash -c`'s command
  argument is not yet implemented. **HIGH realism for a lazy model.** Closed in
  0.9.2.
- **Bare `VAR=val ironlint trust`** (env-var prefix without `env`) — **not
  blocked in 0.9.1.** The `env VAR=val ironlint trust` form IS caught (the `env`
  wrapper is recognized and `VAR=val` assignments skipped), but the semantically
  equivalent bare prefix (`IRONLINT_ROOT=/x ironlint trust`) is not. **MEDIUM-HIGH
  realism.** Closed in 0.9.2.

### Changed

- **`.ironlint.yml` self-check coverage expanded** from 3 macro-ban checks to 6:
  the `todo!`/`unimplemented!`/`dbg!` bans consolidated into one `steps`-based
  check, plus a banned-jargon-in-markdown check, `rustfmt-on-write`,
  `no-trailing-whitespace`, `final-newline`, and `rust-pre-commit`
  (`on: [pre-commit]`: clippy + unit tests). The trust hash changed and was
  re-blessed.
- **`.gitignore`** now excludes machine-local agent install state and tool
  caches (`.pi/`, `.opencode/`, `.agents/`, `.codegraph/`, `.understand-anything/`)
  created by `ironlint init` or local tools.

## [0.8.2] — 2026-07-05 — Watch live-tail motion

### Added

- **`ironlint watch` live-tail motion.** The Stream view now animates: a single
  edit wipes in left-to-right, a burst of writes cascades one row at a time, and
  the newest result draws the eye instead of jump-cutting in. Motion is
  row-quantized to what a terminal can actually render, and the watcher stays
  near-zero CPU when idle — it only ticks fast while something is animating.

### Changed

- **Blocked rows carry a standing dim-red tint** across the full row width,
  including their `└ <check>` detail line, so a block is scannable at a glance
  (internal-error detail lines stay amber). The header dropped its wall clock
  for a static `● PASS` / `● BLOCK` verdict dot.

### Removed

- Dropped the now-unused `chrono` dependency from `ironlint-cli`.

## [0.8.1] — 2026-07-04

### Changed

- **MSRV raised to 1.88.** The locked dependency graph (darling 0.23, image
  0.25, instability 0.3, homedir 0.3 — pulled via ratatui/axoupdater/serde
  tooling) requires rustc 1.88, so 0.8.0's declared 1.87 floor was already
  unbuildable.

### Fixed

- The workspace now builds on its own declared MSRV. 0.8.0's source used the
  unstable `Duration::from_hours` (feature `duration_constructors`, not stable
  at 1.88), so `cargo install`-from-source failed on exactly Rust 1.88 (the
  prebuilt binaries and installers were unaffected); switched to the always-
  stable `Duration::from_secs`. Also made clippy MSRV-aware
  (`clippy.toml msrv`) so it stops suggesting stdlib APIs newer than the floor,
  and added `allow-dirty = ["ci"]` so `dist plan` accepts the hand-hardened
  `release.yml` instead of failing the release.

## [0.8.0] — 2026-07-04 — Readiness hardening

The theme of this release is **fail loud, not silent** — closing the places
where IronLint stopped enforcing without telling anyone, plus the Codex adapter
and broad correctness/DX/security polish from the readiness review.

### Added

- **Codex adapter** (`apply_patch` PreToolUse gate), replacing Reasonix. Codex
  is a `JsonHookSpec` harness like claude-code: `ironlint init --harness codex`
  writes `<project>/.codex/hooks.json` (or `~/.codex/hooks.json` with
  `--global`) — never `config.toml` — registering a `PreToolUse` hook matching
  `apply_patch|Edit|Write`. Unlike the exit-code adapters, the codex hook blocks
  by printing a `permissionDecision:"deny"` JSON object on stdout and exiting
  `0`; malformed stdout on a would-be block fails open, so
  `adapters/codex/hooks/hook.sh` builds every deny payload defensively (`jq`
  with a static fallback). After `ironlint init`, Codex will not run the hook
  until it's reviewed and trusted inside Codex — a manual step unique to this
  adapter. Codex documents `PreToolUse` as a guardrail rather than a complete
  enforcement boundary (a model can route around it via un-intercepted tool
  paths like `unified_exec`); see
  [adapters/codex/README.md](adapters/codex/README.md). Supported harnesses are
  now `claude-code`, `codex`, `pi`, `opencode`.
- **claude-code now gates `MultiEdit` and `NotebookEdit`.** Both previously
  bypassed the hook entirely (the matcher was `Edit|Write`). MultiEdit folds its
  `edits[]` into the final content and checks that; NotebookEdit gates the edited
  cell's proposed source.
- `ironlint trust` prints a summary of exactly what it blesses — config hash,
  every gate file, and every in-repo script referenced by a check's `run:`.
- `ironlint doctor` probes for `jq` / `python3` when a JSON-hook adapter is
  installed (both hooks need them, or they fail open).
- `ironlint validate --format json`, and `ironlint check --require-match`
  (fails CI when a file matches zero checks — a glob-typo guard); the human
  `pass` line now notes when nothing matched.
- `ironlint update` suggests refreshing adapter hooks after a self-update.

### Changed

- **Verdict JSON `SCHEMA_VERSION` 5 → 6.** `GateError` gained a `detail` field
  naming the run command (truncated) and effective timeout, surfaced in the
  human InternalError line as a remediation clause.
- **claude-code Local install** now writes `.claude/settings.local.json` (the
  personal, gitignored file) instead of the committable `.claude/settings.json`,
  so the machine-specific absolute hook path is never committed.
- **Adapter staleness** is now derived from the running binary's embedded
  artifact hashes (the manual `CURRENT_ADAPTER_VERSION` counter is gone);
  `doctor` reports an installed hook as outdated when it differs from the binary.
- Config discovery walks up to the repo root to find `.ironlint.yml`; the
  missing-config error now names the fix (`ironlint init`).
- One consistent CLI error voice (lowercase `error:`); raw `anyhow` cause chains
  no longer leak to users at the CLI boundary.
- Codex hook registration timeout raised 30s → 120s so sequential checks under
  the default per-check cap can't blow the harness budget; per-adapter
  timeout-budget interplay is now documented.
- Refreshed the stale claude-code `plugin.json`.

### Fixed

- `run_gate` no longer overruns the timeout on the pass path when a check's
  descendant escapes the process group (`setsid`/`setpgid`) holding a pipe open
  — the stdout/stderr drain is now bounded.
- Diff parser handles `git mv` renames and C-quoted non-ASCII paths; an
  unrecognized `+++` header now fails closed instead of silently dropping a file.
- pre-commit `$IRONLINT_FILES` entries are absolute; non-UTF8 path bytes are
  preserved faithfully instead of being lossily `display()`-ed.
- Clean UTF-8 decode errors in the claude/codex hooks (a non-UTF8 file yields a
  one-line reason, not a Python traceback).
- Codex adapter fails closed on an op-less `apply_patch` envelope.
- In `--format json` mode, load/trust failures emit a JSON error object instead
  of empty stdout.

### Security

- `$IRONLINT_TMPFILE` is created `O_EXCL` at mode `0600` (was briefly
  world-readable as a repo sibling — a local symlink-race / read window).
- `IRONLINT_TIMEOUT` is floored at 10s, and an ambient override that *shortens*
  the config value is logged — a prompt-injected `IRONLINT_TIMEOUT=1` can no
  longer silently force every check to time out (→ exit 3 → fail-open).

### Performance

- Telemetry `.ironlint/log.jsonl` rotates at 10 MiB (keeps one `.1`).
- `ironlint watch` reads only the appended tail each tick instead of re-reading
  the whole log.

### Removed

- The Reasonix adapter (replaced by Codex) and the manual
  `CURRENT_ADAPTER_VERSION` staleness counter.

## [0.7.0] — 2026-07-01 — Hector is now IronLint

### Changed

- **BREAKING: the tool is renamed Hector → IronLint.** Every user-facing name
  changes, with no compatibility shims — the old names are not read:
  - binary: `hector` → `ironlint` (crates: `ironlint-core`, `ironlint-cli`)
  - config: `.hector.yml` → `.ironlint.yml`; gate scripts dir `.hector/gates/`
    → `.ironlint/gates/`
  - check ABI env vars: `HECTOR_FILE`, `HECTOR_FILES`, `HECTOR_ROOT`,
    `HECTOR_EVENT`, `HECTOR_TMPFILE`, `HECTOR_TIMEOUT`,
    `HECTOR_FAIL_CLOSED_ON_INTERNAL` → the same names under `IRONLINT_*`
  - disable directive: `hector-disable:` → `ironlint-disable:`
  - trust store: `~/.config/hector/trust.json` → `~/.config/ironlint/trust.json`
    (existing configs must be re-blessed with `ironlint trust`)
  - telemetry: `.hector/log.jsonl` → `.ironlint/log.jsonl`
  - installed skills: `hector-config`/`hector-init`/`hector-review` →
    `ironlint-config`/`ironlint-init`/`ironlint-review`
  - repository: github.com/christopherarter/hector →
    github.com/christopherarter/ironlint (the old URL redirects)

  Existing installs: install the `ironlint` binary, re-run `ironlint init` to
  rewire agent hooks, rename `.hector.yml` → `.ironlint.yml` (updating any
  `$HECTOR_*` references), and re-bless with `ironlint trust`. Verdict and
  telemetry JSON schemas are unchanged (both stay at 5).

### Fixed

- Adapter metadata (Claude Code plugin marketplace, opencode/pi package
  manifests, adapter READMEs) pointed at a nonexistent `dynamik-dev` GitHub
  repo — now `christopherarter/ironlint`.
- The opencode and pi adapter test suites still drove the binary with 0.3-era
  `gates:` fixtures, broken since the 0.4 checks pipeline — migrated to the
  `checks:` model (both suites green again in CI).
- `llms.txt` still described the removed 0.2 engine/severity/baseline model and
  linked four doc pages that no longer exist — rewritten against the current
  checks model and docs tree.

## [0.6.0] — 2026-07-01 — init onboarding plan preview

### Changed

- **`ironlint init` previews what it installs, then confirms.** Harness
  onboarding now renders a per-file plan — grouped per harness and tagged
  `detected` (found on this machine) or `requested` (named with `--harness`) —
  showing every hook/plugin file it writes, the settings key it patches, and the
  `ironlint-config` authoring skill it installs, then prompts `Proceed? [Y/n]`.
  This replaces the terse "Install ironlint hooks into …" prompt, which
  understated the footprint (it also installs a skill) and named no paths. The
  two entry paths are unified: explicit `--harness` previously installed
  silently with no preview — it now shows the same plan and confirms. `--yes`
  skips the prompt; `--dry-run` renders the plan and installs nothing. Paths
  display home- and project-relative, and color is applied only when stdout is a
  terminal (piped/CI output stays plain). The `ironlint init --dry-run` output is
  now this plan tree rather than the former flat `write <path>` list.

## [0.5.0] — 2026-06-30 — temp files, `--force`, stack-agnostic init

### Added

- **`$IRONLINT_TMPFILE`.** On a `write` event, when a check's `run`/`steps`
  reference the token, ironlint materializes the proposed content (the bytes
  also delivered on stdin) to a temp file beside `$IRONLINT_FILE` with the same
  extension, exports its absolute path, and removes it after the check. This is
  for file-oriented tools (Biome, ESLint file-mode, `tsc`, ruff) that want a
  real path on disk with the right extension rather than stdin. Creation is
  lazy (only when the check references the token) and write-only — on
  `pre-commit` the files are already on disk at `$IRONLINT_FILES`, so the var is
  unset. Materialization is bounded to the project root unless
  `--allow-external-paths`. Additive to the check ABI — no schema change; a
  check that never mentions the token is unaffected.
- **`ironlint check --force`.** Run the named `--check <id>`(s) against `--file`
  even when the path falls outside their `files` glob — for ad-hoc testing of a
  check against an arbitrary fixture. Scope-only: the check-id filter, inline
  `ironlint-disable` directives, and the `on:` lifecycle all still apply.
  Requires `--check`; exits `1` if passed without it.

### Changed

- **`ironlint init` is stack-agnostic.** It no longer detects the toolchain or
  scaffolds tool-specific checks — the Biome / ESLint / ruff wrappers and the
  Rust/Node grep templates are gone, and so is the stack-detection that fed
  them. `init` now emits one universal baseline — `no-fixme` and
  `no-merge-markers` (both read proposed content from stdin) — plus commented,
  copy-paste examples showing the stdin and `$IRONLINT_TMPFILE` patterns. Harness
  onboarding (installing ironlint's hook into claude-code / pi / opencode /
  reasonix) is unchanged.

## [0.4.0] — 2026-06-29 — checks pipeline redesign

### Breaking

- **Config schema.** The top-level `gates:` key is replaced by `checks:`. Each
  check is `files:` (glob or list) + `run:` (shell command) or `steps:` (a
  sequence of `{name, run}`), plus optional `on:` (lifecycle) and `name:`. Old
  `gates:` configs fail at parse time.
- **Nonzero blocks.** Any nonzero exit (1–125) from a check now blocks. The
  old model required exit `2` to block; exits 0, 1, and 3–125 were passes. The
  natural forbid idiom is now `! grep -nE 'pattern'` (no `exit 2` ceremony).
  Checks that were written to exit `2` continue to block — the change is
  additive for existing scripts.
- **Lifecycles: `on: [write, pre-commit]`.** Default is `on: [write]`. The
  `write` lifecycle fires per file on every agent edit, with proposed content
  on stdin. The `pre-commit` lifecycle fires once per check before a commit,
  with `$IRONLINT_FILES` (newline-joined staged files) and empty stdin.
- **`$IRONLINT_FILES`.** New env var: newline-joined list of all files under
  check (single entry for `write`; all staged files for `pre-commit`).
  `$IRONLINT_FILE` is still set for `write` but not for `pre-commit`.
- **`$IRONLINT_EVENT`** is now `write` or `pre-commit` only. The former
  `edit` and `manual` values are gone.
- **Verdict schema 5 / telemetry schema 5.** `Verdict` and `PerCheckRecord`
  carry the updated lifecycle and check vocabulary. Adapters parsing the
  verdict JSON must accept `schema_version: 5`.

### Added

- **`ironlint watch`** — a read-only live TUI over `.ironlint/log.jsonl` with a
  Stream (newest-first run feed) and an Explorer (per-check health ranked by
  blocks). Built on existing telemetry; no schema change.
- **`ironlint update`.** Self-update to the latest GitHub release. Reads the
  cargo-dist install receipt and, when a newer release exists, downloads and
  re-runs the same installer the binary was installed with, then self-replaces.
  A no-op when already current; exits `1` with channel-specific guidance for
  non-installer builds (Homebrew / `cargo install` / source).
- **`ironlint schema`.** Prints the `ironlint-config` authoring skill to stdout —
  the same guide that `ironlint init` installs for each coding agent.
- **`ironlint init` harness onboarding.** `ironlint init` now installs ironlint's
  hook into detected coding agents with a detect-then-confirm UX. Bare `ironlint
  init` auto-detects installed harnesses (claude-code, reasonix, pi, opencode)
  and prompts before writing anything. New flags: `--harness <name|all>`
  (repeatable; selects explicitly), `--yes` (skip prompt), `--hook-only` (skip
  config scaffold), `--no-hook` (config only, legacy behaviour), `--dry-run`
  (print plan, write nothing), `--uninstall` (remove hook entry + artifacts;
  leaves `.ironlint.yml` and trust store untouched), `--global` (claude-code and
  pi only: write to user-global settings instead of project). Adapter artifacts
  are written atomically to `~/.config/ironlint/adapters/<harness>/`; a
  `.ironlint-adapter.json` sidecar (per-file sha256 + version) tracks installed
  state. Re-runs are idempotent.
- **`ironlint doctor` per-harness adapter status.** `doctor` now reports a row
  for each installed harness inside the existing `checks[]` array — name is the
  harness id (`claude-code`, `reasonix`, `pi`, `opencode`), same
  `{name, status, detail, remediation}` shape as other checks. A registered
  harness with a missing artifact → `fail` → exit 1; modified or outdated
  artifact → `warn`; ok → `pass`; not installed/registered → row omitted.

### Known gaps

- **`docs/` guide tree.** The in-depth guides under `docs/` (getting-started,
  architecture, writing-checks, reference, adapters) still document the 0.3
  `gates:` / exit-2 model. Migration to the 0.4 checks model is a tracked
  follow-up; until then, prefer `README.md` and `ironlint schema` as the
  authoritative references.

## [0.3.0] — 2026-06-25 — gates redesign

### Added

- **Trust:** out-of-repo allow-list at `~/.config/ironlint/trust.json`; `ironlint
  check` fails closed (exit 1) until `ironlint trust` blesses the config +
  `.ironlint/gates/`; `ironlint init` auto-blesses.

### Breaking

- **Config schema.** The top-level `rules:` key is replaced by `gates:`. Each
  gate declares `files:` (glob or list of globs) and `run:` (shell command).
  Old `schema_version`, `engine:`, `severity:`, `skip:`, and `llm:` fields
  are rejected at parse time.
- **Exit-code contract.** Exit 2 = Block (≥1 gate returned exit 2); exit 3 =
  InternalError (≥1 gate crashed: not-found / timeout / signal); exit 0 = Pass.
- **Verdict schema 4 / telemetry schema 3.** `Verdict` no longer carries
  `deferred_rules`, `engine`, or `severity` fields. Telemetry `LogEntry`
  drops `semantic_verdict` and `semantic_skipped` variants.

### Removed

- `engine:`, `severity:`, `baseline:` / `ironlint baseline`, `ironlint migrate`,
  `skip:` — all removed; configs using them fail at load with a pointed error.
- `guide` subcommand folded into `ironlint explain` (now shows gates in scope
  for a file and their run commands).

### Changed

- `ironlint init` success message now advises `ironlint check --file <path>`
  after scaffolding (init auto-blesses, so no separate `ironlint trust` is needed).
- `Explain` subcommand help reworded to gates vocabulary.
- `ExecutionConfig.max_workers` removed (dispatch is sequential; only
  `timeout_secs` is used). Configs that set `execution.max_workers` continue
  to parse (serde ignores unknown fields).

## 0.2 wire-format coordination — superseded by 0.3, never released

> Historical record. The 0.2 line (engines, `llm:`, capability sandboxing, the
> deferred semantic-eval envelope) was developed but never released; the 0.3
> gates redesign above supersedes all of it. Kept for provenance.

This work batched all four contract-shaped changes from
`docs/audits/2026-05-24-check-end-to-end-audit.md` into one CHANGELOG
section so adapters and consumers see them together. Skip to
**Migrating to 0.2** below for the upgrade checklist.

### Removed

> **Supersedes the LLM-related entries below.** The deferred-envelope,
> `semantic`/`session`, and `llm:` work recorded elsewhere in this 0.2
> section was built during the cycle and then removed before release — 0.2
> ships as a static gate. Those entries are kept for development history; the
> bullet here is the net effect.

- **LLM evaluation.** The `semantic` and `session` engines, the `llm:` config
  block, `--emit-semantic-payload`, `--print-prompt`, `check --session`,
  `ironlint session`, and `ironlint record-verdict` are gone. IronLint is a static
  gate: `script` + `ast` only. Configs containing the removed engines fail at
  load with a pointed error naming the rule; `ironlint migrate` drops them with
  a notice. Verdict schema is now 3 (drops `deferred_rules` and the
  `semantic`/`session` engine tags); telemetry schema is now 2.

### Breaking

- **C1 (trust):** the trust fingerprint now canonicalizes through
  `serde_json::Value` (RFC 8259) instead of `serde_yaml`'s emitter.
  `serde_yaml`'s output is not normative — scalar style and indent width
  changed across 0.8/0.9/0.10, so a `cargo update` could invalidate every
  checked-in fingerprint with no actual config change. Every checked-in
  `.ironlint.yml` must be re-signed: `ironlint trust <path>`. Old fingerprint
  mismatch errors now include a re-sign hint. YAML anchors/aliases are
  rejected with a clear error rather than silently hashed.
- **C5 (prompt sentinel):** the deferred envelope's `evaluator_input`
  now wraps trusted-policy and untrusted-evidence in **per-call random
  delimiters** (`<TP-{32 hex}>…</TP-{token}>`, `<UE-{token}>…`). The
  previous fixed `<TRUSTED_POLICY>`/`<UNTRUSTED_EVIDENCE>` tags were
  guessable, letting attacker-supplied content forge a close-tag and
  inject a fake policy section. Anything parsing the prompt structure
  (interpreter skill, `ironlint-evaluator` subagent) must read the
  boundaries from the rendered prompt, not assume the literal tags.
- **`DEFERRED_SCHEMA_VERSION` bumped 2 → 3** (non-additive: `evaluator_input`
  shape changed from a single string built without per-rule context to a
  per-rule structure threading `context: file`/`context: repo`; see B5).

### Added
- **B7 (`Status::InternalError`):** new verdict status + CLI exit code
  **3** for engine-level errors (LLM key missing, AST refused diff,
  script spawn failure, context expansion failure). Previously these
  surfaced as `Block` with a confusing `__internal` violation. Adapter
  policy: default to **allow** on exit 3 (fail-open); opt into
  fail-closed via `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`. Both the
  Claude Code hook (`adapters/claude-code/hooks/hook.sh`) and the
  OpenCode plugin honor this.
- **B4 (deferred warnings):** the deferred envelope's
  `payload.warnings` now carries deterministic Warn-severity violations
  the deferred branch used to drop on the floor. Operators were
  silently losing every script/AST warning whenever the deferred
  branch fired.
- **B3 (`claude-code-subagent` + `engine: session` stop path):**
  `ironlint check --session` under the subagent provider used to print
  *"internal error during session check"* — `check_session`
  hard-required an `LlmClient` and `build_from_config` returns `None`
  for the subagent provider. Now emits a session-aggregate
  `DeferredVerdict` (`file: ""`, `diff: <per-edit framing>`); the
  Claude Code stop hook wraps it in `hookSpecificOutput.additionalContext`
  exactly like the `PostToolUse` per-file branch.

### Changed
- **A1 (baseline)**: file-level violations (`line: None`) now require
  both fingerprint AND normalized body match. The prior behavior turned
  baseline into a per-file disable for passthrough script rules (the
  default since R4). v2 baselines continue to match on fingerprint
  alone during a grace period; run `ironlint baseline record` to
  re-record entries under the new schema. Storage schema bumped v2 → v3.
- **B5 (per-rule context expansion):** the deferred envelope's
  `evaluator_input` is now per-rule. Each rule's slice uses the rule's
  declared `context:` scope (`diff` / `file` / `repo`) — `context: file`
  threads the full file body; `context: repo` threads the canonical
  repo-expansion stub. The previous single-string render meant a
  `context: file` rule would silently see only the diff under the
  subagent route while the direct-API route saw the file. Direct-API
  and subagent now produce byte-identical evidence modulo the C5
  per-call sentinel. Session rules also see per-rule scoped aggregates
  (only edits matching the rule's scope).
### Fixed
- **A2 (diff parser)**: POSIX `diff -u` patches with `\t<timestamp>`
  headers now parse correctly. The previous parser only stripped `\r`,
  so the tab and timestamp landed in the `PathBuf` — every scope match
  silently missed and `ironlint check --diff` returned a clean pass on
  any non-git patch. `build_single_file_diff` had the symmetric bug;
  both call sites are fixed and regression-tested at parser and CLI
  levels.

### Policy
- **C6 (version bump policy):** `SCHEMA_VERSION` only bumps on field
  *removals*, type changes, or semantic re-interpretations — additive
  fields gated by `skip_serializing_if` do not bump. R6's spurious
  `Verdict::SCHEMA_VERSION` 2 → 3 bump (it added the optional
  `deferred_rules` field) is reverted to **2**.
  `DEFERRED_SCHEMA_VERSION` advances to **3** because B5's
  `evaluator_input` shape change is non-additive.

### Migrating to 0.2
1. Pull main and run `ironlint trust <path>` against every `.ironlint.yml`
   in your repo — the trust fingerprint algorithm changed (C1).
2. If you have CI that asserts `Verdict::schema_version == 3`, accept
   `>= 2` instead (C6 reverts the spurious bump). The deferred-envelope
   `schema_version` is now `3`.
3. If your CI parses the deferred envelope, update for v3:
   - `payload.warnings` is now present (B4)
   - `payload.evaluator_input` is per-rule (B5)
   - prompt sentinel tags carry a per-call random suffix (C5)
4. If you script around exit codes, add a case for **3** (engine
   internal error, B7). Default to allow; opt into fail-closed via
   `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`.

### Hook output + capability warning quieted (R7)

- Claude Code adapter hook emits exactly one block message per block — verdict JSON on stderr — confirmed by piping a synthesized `PostToolUse` event through `adapters/claude-code/hooks/hook.sh`. The doubled `PostToolUse:Edit hook returned blocking error` headers seen in the audit transcript came from a second plugin (`bully`) installed alongside `ironlint` in the same Claude Code session, not from IronLint emitting twice. No IronLint-side change required for this half.
- macOS "capability enforcement is best-effort" advisory is no longer printed from `engine::capability::run_best_effort_macos` on every script-rule run. Routine `ironlint check` invocations now keep stderr empty on macOS, both from a terminal and through the adapter hook (which spawns ~3 ironlint processes per edit, bypassing the per-process dedup landed in `f47ef82`).
- The platform-capability story moves to a new `capabilities` doctor row (`ironlint doctor`): `pass` on Linux (CLONE_NEWNET enforces `network: false`); `warn` on macOS and other non-Linux targets with a `docs/security.md` pointer. Library helper `ironlint_core::engine::capability::platform_capability_status()` is the single source of truth.
- Doctor JSON shape stays additive: new `capabilities` row lands between `engines` and `adapter`. Schema is additive-only per `docs/doctor.md`.

### `ironlint init` — workspace & linter detection (R1)

- Scaffolds scopes from the detected workspace shape (`pnpm-workspace.yaml`, `package.json` `workspaces`, Cargo `[workspace] members`, `go.work`). Single-package repos still get `src/**/*.<ext>`; monorepos get per-workspace globs like `apps/**/src/**/*.ts`.
- Detects existing linters (biome, eslint, ruff) and:
  - Skips grep rules that would duplicate the linter (e.g. no `no-console-log` when biome is configured).
  - Scaffolds a passthrough wrapper rule (`biome-check` / `eslint-check`) instead, using the project's package-manager exec command (`pnpm exec` / `yarn exec` / `npx`).
- Always appends a commented-out `llm:` block + example semantic rule so the subagent path is discoverable without docs.
- No flags added — detection is automatic.
- `init.rs` split into a submodule (`commands/init/{mod.rs, detect.rs}`) so detection is testable in isolation.

### Verdict — surface deferred semantic rules on blocked verdicts (R6)

- `Verdict` now carries an optional `deferred_rules: Vec<DeferredRuleRef>` field listing rules that would have been evaluated by the subagent path but were suppressed because a deterministic rule blocked the edit first. Each entry is `{ rule_id, severity, reason }`. Closes the "silently dropped my semantic rule" gap surfaced by the first-run audit (transcript 2).
- `Verdict::SCHEMA_VERSION` bumped `2 → 3` (additive field; envelopes without `deferred_rules` are byte-compatible via `skip_serializing_if = "Vec::is_empty"`).
- Claude Code interpreter skill (`adapters/claude-code/skills/ironlint/SKILL.md`) now surfaces deferred rules in its block summary so users see that their configured semantic rules are alive even when not evaluated this turn.

### LLM config — surface cleanup for `claude-code-subagent` (R2 + R5)

- `llm.model` is now optional when `provider == claude-code-subagent`. Previously it was required-but-ignored. If set, ironlint emits a one-time stderr warning per process noting that the subagent uses the Claude Code session's model.
- New optional `llm.evaluator_model: <model-id>` propagates through the `DeferredVerdict` payload so the Claude Code interpreter skill can dispatch the `ironlint-evaluator` subagent under a specific model (e.g. `haiku` for cheap policy checks). When unset, the subagent's frontmatter `model:` is used. Today Claude Code's subagent dispatch does not accept a per-call model override; the skill surfaces the requested value as an advisory pointing the user at the subagent's frontmatter file. If/when Claude Code adds inline overrides, the skill will pass the value through directly.
- `DEFERRED_SCHEMA_VERSION` bumped to `2` to reflect the new optional payload field. Envelopes without `evaluator_model` are byte-compatible with the prior shape (`skip_serializing_if = "Option::is_none"`).
- **Library-additive only** for direct-API providers (anthropic / openrouter / ollama). Their `model` field stays required.

### Adapters — skip self-check of policy files (R3)

- Both adapters (`adapters/claude-code/hooks/hook.sh`, `adapters/opencode/src/index.ts`) now exit 0 without invoking `ironlint` when the changed file is `.ironlint.yml` or `.bully.yml`. Editing the policy file itself no longer fires the trust gate mid-edit and surfaces a misleading "internal error" to the user.
- Match is by basename, so absolute paths work too.

### Script engine — `output:` default flipped to `passthrough` (R4)

- **Breaking (config):** Per-rule `output:` field default changes from `parsed` → `passthrough`. Existing configs that depended on parsed-mode violation extraction must now set `output: parsed` explicitly. The set of supported parsed formats does not grow — we will not chase a parser per tool.
- Rationale: first real-world test (2026-05-23) showed `parsed` mis-handling biome's pretty diagnostic frame as a chain of false violations. Bully's design is passthrough; we match it.
- `ironlint init` scaffold no longer emits `output: parsed`.

### Subagent semantic-eval — deferred-payload path (H1)

- New CLI flag `ironlint check --emit-semantic-payload` and new config value `llm.provider: claude-code-subagent`. When either is active, `engine: semantic` and `engine: session` rules are collected into a `DeferredVerdict` JSON envelope on stdout instead of being dispatched to the configured LLM. The envelope is byte-compatible with bully's `additionalContext` payload — the Claude Code adapter (H3, separate plan) wraps it for in-session subagent dispatch.
- Exit code semantics unchanged: deterministic block → 2 (deferred suppressed); pass + envelope → 0; pass + no envelope → 0.
- New module `ironlint_core::verdict_deferred` exposes `DeferredVerdict`, `DeferredPayload`, `DeferredRule`, and `DEFERRED_SCHEMA_VERSION` (independent of `Verdict::SCHEMA_VERSION`).
- New helper `ironlint_core::llm::prompt::build_evaluator_input(rules, primary, context)` — concatenates the (system, user) tuple from `build_prompt_split` for inclusion in the envelope's `_evaluator_input` field.
- Wire format was documented in `docs/emit-semantic-payload.md` (removed with the LLM-eval feature).
- **Library-additive only.** No `Verdict` change, no exit-code change. Existing direct-API users (anthropic / openrouter / ollama) are unaffected.

### Subagent semantic-eval — `ironlint record-verdict` (H2)

- New CLI subcommand `ironlint record-verdict --rule <id> --verdict <pass|violation> [--file <path>] [--dir <path>]`. Appends one `LogEntry::SemanticVerdict` record to `.ironlint/log.jsonl` so subagent-evaluated rules show up in coverage reports. Consumed by the Claude Code adapter's interpreter skill (H3, separate plan).
- `--verdict` is a clap `ValueEnum`; invalid values are rejected at parse time.
- First invocation against a fresh log lazily stamps a `session_init` record so the log starts with the canonical first-record type.
- Exit codes: `0` success, `1` telemetry write failure. Never `2` — `record-verdict` is not a gate.
- Wire format and trust model were documented in `docs/record-verdict.md` (removed with the LLM-eval feature).
- **Library-additive only.** No new core surface; reuses `ironlint_core::telemetry::{append, LogEntry::SemanticVerdict}` shipped in D1.

### Subagent semantic-eval — Claude Code adapter mode (H3)

- New Claude Code adapter mode activated by `llm.provider: claude-code-subagent` in `.ironlint.yml`. The `PostToolUse` hook routes through `ironlint check --emit-semantic-payload` (H1) and wraps the resulting `DeferredVerdict` in Claude Code's `hookSpecificOutput.additionalContext` envelope, preamble `AGENTIC LINT SEMANTIC EVALUATION REQUIRED:`. Restores bully's in-session subagent path for Claude Code subscription users — no `ANTHROPIC_API_KEY` required.
- New interpreter skill `adapters/claude-code/skills/ironlint/SKILL.md` activates on the preamble, judges short single-rule payloads inline, dispatches the `ironlint-evaluator` subagent for everything else, applies error-severity fixes via `Edit`, and records each rule's verdict through `ironlint record-verdict` (H2) so coverage telemetry remains accurate.
- New subagent definition `adapters/claude-code/agents/ironlint-evaluator.md` — read-only, returns `VIOLATIONS:` / `NO_VIOLATIONS:` text, no `Read`/`Grep`/`Glob` tools.
- Direct-API mode (anthropic / openrouter / ollama) is unchanged — the hook only diverges when `.llm.provider == "claude-code-subagent"`.
- Plugin version bumped 0.1.0 → 0.2.0.
- Adapter README documents both modes and the `model:` placeholder requirement.

### Script engine — `output: parsed | passthrough` (E2)

- New per-rule `output:` field on `Rule`. `Parsed` (default) feeds the chosen stream through `engine::output::parse`, which extracts `file:line:col: msg` structure from canonical lint output (clippy `--message-format short`, `ruff`, `eslint --format compact`) and the `grep -n` `<line>:<text>` shape — populating `Violation.line` / `Violation.column`. `Passthrough` preserves the 0.1 behaviour: stdout+stderr land verbatim in `message` with `line: None`.
- Parsed mode emits one `Violation` per record, so a multi-hit lint run no longer collapses into a single concatenated message.
- **Breaking (library):** `engine::script::run_script_rule` now returns `Result<Vec<Violation>>` (was `Result<Option<Violation>>`). The trait impl was already vec-shaped; only direct callers of the free function change.
- New parser guard: `file:line: msg` mode now requires a path separator in the file capture, so `example.com:42: msg` and `grep -n` `<line>:<text>` no longer mis-parse as `{ file: "example.com", line: 42 }`. Windows drive paths (`C:\foo.rs:14:5: msg`) parse correctly.

### OpenCode adapter — pre-flight gating

- The adapter now hooks `tool.execute.before` (was `.after`) and shadow-writes the proposed file content before invoking `ironlint check --file`, then restores the pre-edit state regardless of verdict. A `block` verdict throws so opencode never executes the tool — previously the write had already landed before ironlint saw it.
- `tool.execute.after` is still used for `ironlint session record` (best-effort cross-edit tracking).
- Late-init fix: hooks register unconditionally and re-check `.ironlint.yml` per invocation, so `ironlint init` mid-session starts gating without an opencode restart.
- Recognises opencode's native `find` / `replace` / `replaceAll` edit-arg shape (with legacy `oldString` / `newString` as fallback for older opencode versions).
- Module exposes both `default` and named `IronLintPlugin` exports so neither loader pattern silently no-ops.

### Capability sandbox — macOS warning dedup

- The "capability enforcement is best-effort on this platform" stderr line now fires at most once per process (was: once per script rule invocation). Extracted into a testable `should_warn_macos_with` helper.

### Telemetry — typed records (D1)

- `.ironlint/log.jsonl` now carries typed records: `session_init`, `check`, `semantic_verdict`, `semantic_skipped`. Each line has a `type` discriminator. Per-rule outcomes (`PerRuleRecord`) are nested under `Check.rules` instead of being one-line-per-(file,rule). `ironlint_version` and a telemetry `schema_version` are stamped in every `session_init`.
- **Backwards compat:** `ironlint_core::telemetry::read_all` accepts the pre-D1 flat shape via an untagged fallback and lifts each line into the closest typed variant. A one-time stderr deprecation warning fires per process when the fallback is used. The fallback will be removed at the 0.3 verdict freeze.
- New CLI subcommand `ironlint session start` stamps a `session_init` record explicitly. `ironlint session record` stamps one lazily on its first invocation per session.
- **Breaking (library):** `pub enum LogEntry` replaces `pub struct LogEntry` in `ironlint_core::telemetry`. Pre-1.0; consumers using the writer should migrate to constructing the appropriate variant.
- Wire format documented in [`docs/operating/telemetry.md`](docs/operating/telemetry.md).

## 0.1b — Engine set complete

### Engines
- `ast`: structural pattern matching via `ast-grep-core`. Rules specify `pattern:` and `language:`.
- `semantic`: LLM-evaluated plain-English rules. Requires an `llm:` block. Anthropic provider only at 0.1b.
- `session`: cumulative-changeset rules fired by `ironlint check --session`. Useful for "auth changed but no tests" type rules.

### Commands
- `ironlint init`: detect stack, scaffold a starter `.ironlint.yml`.
- `ironlint migrate`: rewrite `.bully.yml` → `.ironlint.yml`.
- `ironlint baseline`: record current violations, silence them from future runs.
- `ironlint check --session`: evaluate session rules and clear `.ironlint/session.json`.

### Internals
- `RuleEngine` trait for unified engine dispatch.
- `LlmClient` trait + `AnthropicClient` impl.
- `IronLintEngine::builder()` to inject LLM dependencies.
- `IronLintEngine::check` returns `Result<Verdict>` (engine errors surface as `engine: trust` violations).
- Telemetry log at `.ironlint/log.jsonl`.

### Preflight fixes from 0.1a review
- Configs with unimplemented engines fail at load time (no silent passes).
- Invalid scope globs fail at load time.
- `// ironlint-disable:` comments now silence violations when line numbers are present.
- `--diff` mode plumbs the diff through to script rules.
- `.bully.yml` configs print a deprecation warning.

## Coming in 0.1c / Plan C
- Claude Code adapter (plugin.json, PostToolUse + Stop hooks, skills ported from bully).
- `CheckInput::Staged` (git index).
- Full repo-context expansion.
