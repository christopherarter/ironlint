# Phase 5 — Docs & remaining polish (backlog)

**Goal:** everything else the review surfaced — documentation fixes, smaller
correctness/DX/security refinements, and hardening. These are lower-severity than
Phases 1–4 but they are what separates "good" from "the uv/ruff tier."

**Read [`README.md`](README.md) first.** The repo rules still apply: **code
tasks start with a failing test**, meet the ≤15 complexity + ≥80% coverage
gates, and end with Standard Verification. **Doc/config tasks are exempt from the
test rule** (they say `[doc]` / `[config]` below) but should still be verified by
inspection or a manual run.

These are more compact than earlier phases — each item gives the finding, the
files/anchor, the fix, and how to verify. If any item is unclear, open the
published review report for the full write-up (finding IDs match).

Do them in any order unless a **Depends on** is noted. Group by category so you
can batch similar work.

---

## A. Documentation & information architecture

### 5.1 — Fix the `init` stack-detection docs (removed in 0.5.0)  [O1] `[doc]`
- **Files:** `docs/reference/cli.md` (the `ironlint init` section), `docs/getting-
  started.md` (~line 67), `adapters/claude-code/skills/ironlint-init/SKILL.md`
  (frontmatter description, the `stack-detection` tag, the Step-1 manifest→stack
  table).
- **Fix:** these three surfaces say `init` "detects your stack (Rust/Node/Python)
  and linters (biome/eslint/ruff)" — but 0.5.0 made `init` **stack-agnostic**
  (confirm via `CHANGELOG.md` 0.5.0 and by reading the current init code, which
  emits a universal baseline, not stack wrappers). Rewrite all three to describe
  the real behavior: "emits a stack-agnostic baseline plus commented stdin /
  `$IRONLINT_TMPFILE` examples; wrap your own linters via the `ironlint-config`
  skill." Bump the skill's version so `doctor`'s artifact-hash check flags stale
  installs.
- **Verify:** `grep -rni "detects your stack\|stack-detection\|manifest" docs/
  adapters/claude-code/skills/` returns only accurate, updated text.

### 5.2 — Bring the "Anatomy of a check" page to the 0.4 model  [O2] `[doc]`
- **Files:** `docs/writing-checks/README.md` (~line 14 "exactly two fields",
  ABI table ~line 47), `docs/adapters/README.md` (ABI table), `docs/architecture.md`
  (ABI node).
- **Fix:** the canonical page teaches the pre-0.4 model — "a check has exactly two
  fields," no `steps`/`on`/`name`, and its ABI table omits `$IRONLINT_FILES`.
  Update to the full `Check { files, run|steps, on, name }` and the 5-variable
  ABI. **The correct text already exists** in `adapters/shared/ironlint-config/
  SKILL.md` (which `ironlint schema` embeds) — crib from it. Fix the other two
  drifted ABI tables to match.
- **Verify:** all three ABI tables list `$IRONLINT_FILE`, `$IRONLINT_FILES`,
  `$IRONLINT_ROOT`, `$IRONLINT_EVENT`, `$IRONLINT_TMPFILE`, and stdin; the anatomy
  page documents `steps`, `on`, and pre-commit.

### 5.3 — Grow the cookbook to real stacks  [O3] `[doc]`
- **Files:** `docs/writing-checks/recipes.md` (or a new `recipes/` dir).
- **Fix:** today it has ~6 patterns and 2 concrete tools. Add copy-paste recipes
  for the top ~10: **ruff, eslint, tsc, prettier, clippy, gofmt, black, pytest,
  biome, shellcheck**. For each, make the stdin-vs-`$IRONLINT_TMPFILE`-vs-whole-
  tree decision **for the reader** (e.g. `tsc`/`clippy` are whole-project → show
  the `on: [pre-commit]` + `$IRONLINT_FILES` form; `ruff`/`prettier` accept
  stdin → show the stdin form). Resolve the tension the review noted: the README
  names `clippy`/`tsc` as examples while the init skill says "skip repo-wide
  tools" — give each an explicit whole-tree recipe.
- **Verify:** a Python and a Go dev can copy a working check for their linter
  without further research.

### 5.4 — Write the comparisons page ("why not write the hook myself?")  [O4] `[doc]`
- **Files:** new `docs/comparisons.md`; link from README.
- **Fix:** the #1 objection is never answered. Assemble the scattered answers into
  one page: **vs a hand-rolled hook** (one ABI across N harnesses, the trust
  store, `$IRONLINT_TMPFILE` materialization, telemetry, the write-time
  inversion), **vs lefthook/husky**, **vs pre-commit**. Reuse the README's honest
  "absorbed / declined / added" framing.
- **Verify:** the page directly answers "why not a 5-line PostToolUse hook."

### 5.5 — Add a CI-usage guide + monorepo guidance  [O5] `[doc]`
- **Files:** new `docs/operating/ci.md` (if not already created by Task 4.2),
  a monorepo section in `docs/configuring/targeting-files.md`.
- **Fix:** show running the same `.ironlint.yml` in GitHub Actions/GitLab on the
  PR diff, including the trust-in-CI step; add a monorepo layout section
  (root config + per-package `extends`, or globbed scopes).
- **Verify:** the "local CI" promise round-trips to a documented CI recipe.
- **Depends on:** overlaps Task 4.2 — if 4.2 wrote `ci.md`, this is just the
  monorepo section.

### 5.6 — Move orphaned internal artifacts out of `docs/`  [O6] `[doc]`
- **Files:** `docs/superpowers/{plans,specs}/`, `docs/audits/`.
- **Fix:** ~44k words of internal plans/audits sit unlinked beside the curated
  user docs (and contain phantom env vars / a `schema_version: 2` example that
  crawlers and LLMs pick up as if real). Move them to the sanctioned repo-root
  homes: `git mv docs/superpowers/plans/* plans/` and `.../specs/* specs/` (or an
  `archive/` subdir), and `docs/audits/` → a repo-root `audits/` (or delete the
  5-week-stale launch-readiness one). Leave `docs/` purely user-facing.
- **Verify:** `docs/` contains only the six user-facing dirs; nothing under it is
  an internal plan/spec/audit.

### 5.7 — Add CONTRIBUTING, SECURITY, issue templates, a demo  [O7, S8] `[doc]`
- **Files:** new `CONTRIBUTING.md`, `SECURITY.md`, `.github/ISSUE_TEMPLATE/*`,
  `.github/PULL_REQUEST_TEMPLATE.md`, a demo asset in README.
- **Fix:**
  - `CONTRIBUTING.md`: a human-facing wrapper over `AGENTS.md` (build, test,
    coverage, the failing-test rule).
  - `SECURITY.md` **(also Finding S8)**: a private disclosure contact + a
    coordinated-disclosure window; link the honest threat model in
    `docs/security/trust.md`.
  - Issue forms (bug/feature) that ask for `ironlint doctor --format json`
    output; a PR template.
  - A **demo GIF/asciinema** of the block→fix loop (record with `charmbracelet/
    vhs` so it is reproducible); embed under the README hero.
- **Verify:** the files exist; GitHub renders the issue forms; the README shows
  the demo.

### 5.8 — Retitle the shipped skills off the bully-era name  [D6] `[doc]`
- **Files:** `adapters/claude-code/skills/ironlint/SKILL.md` (H1 "Agentic Lint"),
  and the `metadata.author: dynamik-dev` across the four shipped skills.
- **Fix:** the installed skill (injected into users' agent context) is still
  titled "Agentic Lint" (bully's vocabulary) with `dynamik-dev` author while the
  repo is `christopherarter/ironlint`. Retitle to IronLint, fix author metadata,
  bump skill versions so `doctor` flags stale installs. (Note: this is *bully*
  residue, not Hector — the Hector→IronLint rename was otherwise clean.)
- **Verify:** `grep -rn "Agentic Lint\|dynamik-dev" adapters/*/skills/` is clean.

---

## B. Correctness & ABI polish

### 5.9 — pre-commit `$IRONLINT_FILES` must be absolute  [R3]
- **Files:** `crates/ironlint-core/src/runner.rs` (pre-commit dispatch, where
  parsed relative paths become `GateEnv.files` ~lines 638–653; write-mode
  absolutizes ~588; the ABI promise is in `gate.rs` ~line 19).
- **Fix:** the ABI promises newline-joined **absolute** paths, but pre-commit
  passes relative ones (write mode is absolute). Absolutize the pre-commit set
  (join against the config dir / project root) before dispatch.
- **Test first:** an ABI test symmetric to the existing
  `ironlint_file_is_absolute_for_checks`, asserting each entry of pre-commit
  `$IRONLINT_FILES` is absolute. Fails today.

### 5.10 — Diff parser: handle renames and git-quoted paths  [R9]
- **Files:** `crates/ironlint-core/src/diff/parser.rs` (~lines 50–116).
- **Fix:** a pure `git mv` (no `---`/`+++` pair) and C-quoted non-ASCII headers
  (`"b/caf\303\251.rs"`, the default under `core.quotePath=true`) are silently
  dropped, bypassing pre-commit. Parse `rename from`/`rename to` headers; unquote
  C-quoted paths; make an **unrecognized** `+++ ` form a hard parse error (fail
  closed), not a silent skip.
- **Test first:** a diff with a rename into a gated dir → the file is in the
  changed set; a diff with a quoted non-ASCII path → parsed correctly; a malformed
  header → error. All fail today.

### 5.11 — `$IRONLINT_FILES` join: use OsStr bytes, not lossy `display()`  [R10]
- **Files:** `crates/ironlint-core/src/engine/gate.rs` (~line 74,
  `.map(|p| p.display().to_string())`).
- **Fix:** `display()` turns non-UTF8 path bytes into U+FFFD (a path that won't
  `stat`), while single-file `$IRONLINT_FILE` gets the faithful `OsStr`. Join raw
  `OsStr` bytes instead. Document the newline restriction, or add a NUL-joined
  `$IRONLINT_FILES0` variant for scripts that need robustness.
- **Test first:** a file set including a non-UTF8 path name is passed faithfully
  (platform-dependent; `#[cfg(unix)]`).

---

## C. Security polish

### 5.12 — Floor `IRONLINT_TIMEOUT` / ignore ambient override in enforced mode  [S2]
- **Files:** `crates/ironlint-core/src/runner.rs` (`resolve_timeout` ~line 211).
- **Fix:** `IRONLINT_TIMEOUT=1` from the ambient env (a prompt-injected agent, a
  repo `.envrc`) forces every real check to time out → exit 3 → fail-open. Floor
  the value at a sane minimum (e.g. ≥10s) **and/or** log loudly when an ambient
  override *shortens* the config value. (A stricter "config-only, ignore ambient"
  mode belongs to the future govern product — a floor + log is enough here.)
- **Test first:** the refactored pure `resolve_timeout` (see Task 3.8 item 3)
  clamps `"1"` up to the floor and emits the log.
- **Depends on:** overlaps Task 3.8 (which extracts `resolve_timeout` as a pure
  fn) — do that first if not done.

### 5.13 — `$IRONLINT_TMPFILE`: create `O_EXCL` at mode 0600  [S7]
- **Files:** `crates/ironlint-core/src/runner.rs` (`maybe_materialize_tmpfile`
  ~line 503, the `std::fs::write` of the temp sibling ~line 531).
- **Fix:** proposed (not-yet-saved) content is briefly written world-readable
  (~0644) as a repo sibling, readable by any co-located process; predictable name
  + non-exclusive create is a local symlink-race. Create via an `O_EXCL`,
  mode-0600 open (e.g. `OpenOptions::new().write(true).create_new(true).mode(0o600)`
  under `#[cfg(unix)]`). Keep the in-repo placement (needed for extension-sensitive
  tools) but lock down perms/creation.
- **Test first:** on Unix, assert the materialized tmpfile's mode is 0600.

### 5.14 — Rotate the leaked `.env` key  [S9] `[action]`
- **Files:** repo-root `.env` (untracked).
- **Fix:** a live `OPENROUTER_API_KEY` sits in the untracked `.env` (correctly
  gitignored, never committed, but read by tooling during the review). **Rotate
  the key** and move it out of the repo tree. Confirm no tool loads a repo-root
  `.env` into the check environment (Task 3.4's env scrub covers checks; verify
  nothing else does).
- **Verify:** the old key is revoked; `.env` is no longer at the repo root (or
  contains only a rotated, non-sensitive value).

---

## D. CLI & DX polish

### 5.15 — Upward config discovery + fix the "canonicalizing" error  [D1]
- **Files:** `crates/ironlint-cli/src/commands/*` (config resolution for
  `check`/`validate`/`explain`), and the error text (search for `canonicalizing`
  / `resolving` `.ironlint.yml`).
- **Fix:** from a subdirectory, commands fail with `canonicalizing .ironlint.yml:
  No such file` — no walk-up, and "canonicalizing" is implementation vocabulary.
  Walk up to the repo root (stop at `.git`) to find the default `.ironlint.yml`;
  rewrite the missing-config error to name the fix: "no `.ironlint.yml` found in
  <dir> or any parent — run `ironlint init`" (mirror the good message `doctor`
  already gives). Unify the two verbs ("canonicalizing"/"resolving").
- **Test first:** running `validate` from a subdir of a configured repo succeeds;
  a no-config repo gives the `ironlint init` pointer. Both fail today.

### 5.16 — `validate --format json`; emit JSON errors in JSON mode  [D2]
- **Files:** `crates/ironlint-cli/src/cli.rs` (add `--format` to `validate`),
  `commands/validate.rs`, and the error path in `commands/check.rs` (untrusted/
  load errors must emit a JSON object when `--format json` was requested).
- **Fix:** `validate` (the natural CI policy-lint entry) has no JSON; and in JSON
  mode a load/trust failure emits nothing on stdout + a human `ERROR:` on stderr,
  so a JSON consumer gets an empty document. Add JSON to `validate`; on load/trust
  failure with `--format json`, emit `{"status":"error","reason":"..."}` to
  stdout. Optionally converge addressing (`--dir` vs `--config`) — lower priority.
- **Test first:** `validate --format json` on a good and a bad config emits valid
  JSON with the right status; `check --format json` on an untrusted config emits a
  JSON error object, not empty stdout.

### 5.17 — `pass` should say when nothing matched  [D4]
- **Files:** `crates/ironlint-cli/src/commands/check.rs` (`fn emit`, human path).
- **Fix:** a file with zero in-scope checks prints bare `pass` (identical to a
  real all-passed run), so a glob typo silently disables policy. Print `pass (no
  checks matched <file>)` when the matched set was empty. Add an optional
  `--require-match` that makes "no checks matched" a nonzero exit for CI.
- **Test first:** `check` on a non-matching file prints the "no checks matched"
  note; `--require-match` makes it nonzero.

### 5.18 — InternalError output names the command, timeout, and fix  [D5]
- **Files:** `crates/ironlint-cli/src/commands/check.rs` (`fn emit`, error path),
  `crates/ironlint-core/src/verdict.rs` (`GateError` — add a `detail` field →
  **schema bump to 6**, so update `SCHEMA_VERSION`, the JSON docs, and the
  schema-version tests).
- **Fix:** `error: [x] (timeout)` / `(not_found)` never says the effective
  timeout or which command was missing. Add a `detail` field to `GateError`
  carrying the run command (truncated) + effective timeout; render it in the human
  line with a remediation clause. **Because this changes the verdict JSON shape,
  bump `SCHEMA_VERSION` 5→6** and update every pinning test + `docs/reference/
  verdict-json.md`.
- **Test first:** an internal-error verdict includes the command + timeout in both
  human and JSON output; the schema-version test asserts 6.
- **Note:** this is the one Phase-5 item that touches a locked surface — do it
  deliberately, or defer it if you want to avoid a schema bump right now.

### 5.19 — One error voice; drop raw anyhow chains at the CLI boundary  [D6]
- **Files:** `crates/ironlint-cli/src/commands/*`, `main.rs`.
- **Fix:** custom `ERROR:`, raw anyhow `Error: … Caused by:` chains, and clap
  `error:` all coexist. Standardize on one reporter with a lowercase `error:`
  prefix and a fix-naming clause where known; stop leaking raw `Caused by:` chains
  to users at the CLI boundary. Also sweep the shipped `adapters/claude-code/
  hooks/hook.sh` for dead vocabulary (a pre-0.3 `trust:` key reference, a
  `.bully.yml` skip, internal audit IDs "R3"/"P0-4").
- **Verify:** error messages share one grammar; `grep -n "bully\|trust:" adapters/
  claude-code/hooks/hook.sh` shows only intentional references.

---

## E. Adapter polish

### 5.20 — Document / handle timeout-budget collisions  [E6]
- **Files:** `crates/ironlint-core/src/adapter/registry.rs` (reasonix `timeout:
  30000` ~line 56), `docs/adapters/*`.
- **Fix:** the reasonix hook timeout (30s) equals one check's default cap, and
  checks run sequentially — two slow checks exceed it and the harness kills the
  hook → ungated with no signal. Size harness timeouts from the armed checks'
  caps at init (or set generously), and **document the interplay per adapter**
  (currently zero docs mention it). Consider a lower default cap for
  `write`-lifecycle checks.
- **Verify:** each adapter's timeout is documented relative to `execution.
  timeout_secs`; the numbers no longer collide by default.

### 5.21 — Version-stamp installed adapters; refresh on update  [E7]
- **Files:** `crates/ironlint-core/src/adapter/mod.rs` (`CURRENT_ADAPTER_VERSION`
  ~line 25), `adapter/ops.rs` (sidecar hash compare ~line 377),
  `crates/ironlint-cli/src/commands/update.rs`, `adapters/claude-code/.claude-
  plugin/plugin.json` (`"version": "0.2.0"`, pre-0.4 description).
- **Fix:** staleness detection hinges on a manual `CURRENT_ADAPTER_VERSION` that
  has been `1` since introduction while hooks changed materially; the sidecar
  compares against **install-time** hashes, never the running binary's embedded
  artifact. Compute "current" by hashing the **embedded** artifacts vs the sidecar
  (deletes the manual bump); make `ironlint update` suggest/offer `init
  --hook-only` to refresh hooks. Update the stale `plugin.json` version +
  description.
- **Test first:** doctor reports "stale" when an installed hook differs from the
  running binary's embedded artifact.

### 5.22 — Local claude-code install: write to `settings.local.json`  [E8]
- **Files:** `crates/ironlint-core/src/adapter/registry.rs` (~line 61,
  `settings_local` → project `.claude/settings.json`), `adapter/ops.rs` (~line 94,
  the absolute hook path).
- **Fix:** the Local install patches the **committable** `.claude/settings.json`
  with a machine-specific absolute hook path (`/Users/<name>/…`) — commit it and
  teammates get per-edit "No such file" hook errors + a leaked username. Patch
  `.claude/settings.local.json` (the personal, gitignored file) for Local scope,
  or resolve a portable `ironlint claude-hook` shim via PATH.
- **Test first:** a Local claude-code install writes to `settings.local.json`, not
  the committable file; the path is portable or in the personal file.

### 5.23 — Guard adapter payload parsing; probe dependencies  [E9]
- **Files:** `adapters/reasonix/hooks/hook.sh` (payload parse + the python
  `UnicodeDecodeError` guard ~lines 130–135), `adapters/claude-code/hooks/
  hook.sh`, `crates/ironlint-cli/src/commands/doctor.rs`.
- **Fix:** malformed hook JSON dies at the first `jq` under `set -e` (exit 5 — an
  undefined code); missing `jq` → exit 127 silent fail-open; a non-UTF8 file on
  reasonix `edit_file` throws an uncaught `UnicodeDecodeError` → hard false-block
  with a traceback as the agent message. Add an explicit fail-open guard around
  payload parsing (one-line reason), catch decode errors → skip the gate, and add
  `doctor` dependency probes (`jq`, `python3`) for installed adapters.
- **Verify:** malformed JSON → graceful fail-open with a reason; missing dep →
  doctor warns; non-UTF8 edit → skip, not a traceback block.

### 5.24 — Cover the missing write-tools (NotebookEdit, multi_edit)  [E10]
- **Files:** `adapters/claude-code/hooks/*` (matcher `Edit|Write`),
  `crates/ironlint-core/src/adapter/registry.rs` (reasonix `multi_edit`), the
  reasonix example settings, docs.
- **Fix:** claude-code's `Edit|Write` matcher misses **`NotebookEdit`** (edits via
  it bypass the gate); reasonix `multi_edit` is *registered* but the hook no-ops
  it. Document the `NotebookEdit` gap (and add it to the matcher if the payload
  shape is handleable); implement `multi_edit` folding (tracked in the reasonix
  spec §9.3); align the example settings file's third `write_file|edit_file`
  variant.
- **Verify:** a NotebookEdit payload is either gated or explicitly documented as
  unsupported; a reasonix multi_edit payload is gated, not silently allowed.

---

## F. Testing & release hardening

### 5.25 — Fuzz the two parse surfaces  [T3]
- **Files:** new `fuzz/` (cargo-fuzz) targets for the config parser and the
  unified-diff parser.
- **Fix:** both consume agent-generated untrusted input; a diff-parser panic →
  exit 3 → fail-open → silent bypass. Add two `cargo-fuzz` targets asserting "no
  panic" on arbitrary bytes.
- **Verify:** `cargo +nightly fuzz run config_parser` and `... diff_parser` run
  clean for a short session; wire a short fuzz smoke into CI if cheap.

### 5.26 — Property-test scope matching  [T3]
- **Files:** a `proptest` module for `crates/ironlint-core/src/config/scope.rs`.
- **Fix:** `scope.rs`'s deliberate bare-pattern divergence (`*.py` ≡ `**/*.py`) is
  a textbook invariant. Add proptests: a bare `*.ext` matches that extension at
  any depth; a `dir/**` pattern matches only under `dir`; matching is stable under
  path normalization.
- **Verify:** proptests pass; they would catch a regression in the bare-pattern
  rule.

### 5.27 — Release smoke verification  [T4]
- **Files:** `.github/workflows/release.yml`.
- **Fix:** `pr-run-mode = "plan"` means artifacts are never built before the tag,
  and nothing installs an artifact and runs it; the self-update flow is untested.
  Add a post-tag (or tag-triggered) smoke job that runs the shell installer into a
  temp prefix on each OS and executes `ironlint --version` + a one-check run.
- **Verify:** the smoke job runs on a release-candidate tag and passes.
- **Depends on:** coordinate with Tasks 3.6/3.7 (same workflow files).

### 5.28 — Refresh mutation-testing evidence  [T5]
- **Files:** delete `mutants.out/` and `mutants.out.old/` (stale, May-12,
  pre-0.4-redesign); optionally add a PR-scoped `cargo mutants --in-diff` note to
  `AGENTS.md`.
- **Fix:** run `cargo mutants --file crates/ironlint-core/src/engine/gate.rs`
  (and `runner.rs`, `trust.rs`) once post-0.7; treat survivors in `classify()` as
  confirmations of the Task 3.8 gaps and add tests. Delete the stale dirs.
- **Verify:** stale dirs gone; a fresh mutants run over the engine trio has no
  unexpected survivors in code you own.

---

## G. Performance (beyond spec P1–P8)

### 5.29 — Fast-path prefilter + telemetry rotation + incremental watch  [P3, P4]
- **Files:** `crates/ironlint-core/src/runner.rs` (a cheap path-vs-globs
  prefilter before `trust::ensure_trusted` + full load), `telemetry.rs` (no
  rotation), `crates/ironlint-cli/src/commands/watch.rs` (~line 359, full re-read
  each 250ms tick).
- **Fix:**
  - **Prefilter (P3):** before the full parse + trust hash, cheaply check whether
    *any* check's globs could match the path; if none can, exit pass fast. (Be
    careful: this must not skip the trust check in a way that changes security
    semantics — a non-matching file legitimately runs nothing, so a fast pass is
    correct. Confirm no telemetry/verdict contract is violated.)
  - **Telemetry rotation (P4):** size-based rotation of `.ironlint/log.jsonl`
    (→ `.1`, keep N) so it does not grow unbounded.
  - **Incremental watch (P4):** `watch` remembers a byte offset and parses only
    the new tail each tick instead of re-reading the whole file.
- **Test first (per sub-item):** prefilter returns pass without spawning when no
  glob matches; rotation caps file count; watch tail-parse yields the same rows as
  a full parse for appended lines.
- **Note:** the biggest perf win (dropping `synthesize_diff.sh` on the write path,
  P1/P2) is folded into **Task 3.1** (PreToolUse migration via `--content -`) —
  do that there, not here.

### 5.30 — (tracking) Adapter latency + large-write robustness  [P1, P2]
- **Status:** **handled by Task 3.1.** Migrating claude-code to `PreToolUse` +
  `--content -` deletes the per-edit `synthesize_diff.sh` (bash + awk ×2) that
  roughly doubles latency (~74ms E2E vs ~25ms ironlint) and wedges on 100k-line
  writes (argv `ARG_MAX` cliff). This item is here only so the finding is not
  lost — verify after 3.1 that the write path no longer invokes
  `synthesize_diff.sh` and handles a 100k-line payload without hanging.
- **Verify:** feed a 100k-line write payload through the (post-3.1) claude-code
  hook in a scratch dir — it completes promptly, no hang, no orphaned processes.

---

## H. Additional backlog (lower priority — captured so nothing is lost)

### 5.31 — `ironlint trust` should show what it blesses  [S3]
- **Files:** `crates/ironlint-cli/src/commands/trust.rs`, `crates/ironlint-core/
  src/trust.rs` (`TrustEntry` stores only `{hash, blessed_at}`).
- **Fix:** `trust` is a blind bless — it prints nothing about the shell it is
  vouching for. Make it print the resolved checks (each `run`/`steps` and each
  referenced in-repo script, flagging out-of-gates targets — dovetails with Task
  3.3), and on a **re-bless** show a diff/summary vs the stored hash's snapshot;
  require a confirmation (respect a `--yes` flag for automation). This is the
  "review what you run" guarantee the trust store is supposed to provide.
- **Test first:** `trust` on a config prints its check commands; re-blessing after
  an edit surfaces that something changed.
- **Depends on:** best done after Task 3.3 (so referenced scripts are known).

### 5.32 — Additional harness adapters: Gemini CLI, Copilot  [M5]
- **Files:** new `adapters/gemini/`, `adapters/copilot/` (follow the Task 4.1
  Cursor adapter as the template), registry/init/doctor/docs, tests.
- **Fix:** Gemini CLI (hooks GA; free-tier funnel) and GitHub Copilot (56%
  enterprise; `preToolUse` approve/deny) are both hook-capable. **Cheap win to
  check first:** Copilot can read hooks from `.claude/settings.json`, so the
  existing claude-code artifacts may partially work — verify and document that
  before building a full adapter. Verify each harness's current hook contract
  against upstream docs before building (they change).
- **Priority:** after Cursor (Task 4.1), which is the higher-adoption target and
  the template.

### 5.33 — List adapters in harness-native marketplaces  [M6]
- **Files:** the already-built Claude Code plugin (`adapters/claude-code/.claude-
  plugin/`), plus submissions to community directories.
- **Fix:** discovery for hook tooling happens inside each harness's plugin
  ecosystem; IronLint's docs say "once published to the plugin marketplace…"
  (future tense). Publish the Claude Code plugin; submit to the awesome-lists /
  hook directories. Do the same for Gemini extensions once that adapter (5.32)
  lands.
- **Priority:** low; do after the plugin is confirmed working end-to-end.

**Additional backlog checklist**
- [ ] 5.31 trust shows what it blesses · [ ] 5.32 Gemini/Copilot adapters ·
  [ ] 5.33 marketplace listings

---

## Phase 5 checklist

**Docs/IA**
- [ ] 5.1 init stack-detection docs · [ ] 5.2 anatomy page · [ ] 5.3 cookbook ·
  [ ] 5.4 comparisons · [ ] 5.5 CI/monorepo docs · [ ] 5.6 move orphaned docs ·
  [ ] 5.7 CONTRIBUTING/SECURITY/templates/demo · [ ] 5.8 retitle skills

**Correctness/ABI**
- [ ] 5.9 pre-commit FILES absolute · [ ] 5.10 diff renames/quoted paths ·
  [ ] 5.11 FILES OsStr join

**Security**
- [ ] 5.12 IRONLINT_TIMEOUT floor · [ ] 5.13 tmpfile 0600/O_EXCL ·
  [ ] 5.14 rotate .env key

**CLI/DX**
- [ ] 5.15 upward config discovery · [ ] 5.16 validate JSON · [ ] 5.17 pass
  no-match · [ ] 5.18 InternalError detail (schema bump) · [ ] 5.19 one error voice

**Adapters**
- [ ] 5.20 timeout budgets · [ ] 5.21 version-stamp/refresh · [ ] 5.22
  settings.local.json · [ ] 5.23 payload guards/dep probes · [ ] 5.24
  NotebookEdit/multi_edit

**Testing/release**
- [ ] 5.25 fuzz parsers · [ ] 5.26 proptest scope · [ ] 5.27 release smoke ·
  [ ] 5.28 refresh mutants

**Performance**
- [ ] 5.29 prefilter + telemetry rotation + incremental watch · [ ] 5.30 (verify
  after 3.1)
