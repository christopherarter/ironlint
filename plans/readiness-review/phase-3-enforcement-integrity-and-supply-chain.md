# Phase 3 — Enforcement integrity & supply chain

**Goal:** make the gate actually enforce (pre-write, no silent bypass, no RCE via
un-hashed scripts, no secret leakage) and bring the release/CI pipeline up to the
bar expected of a trust-branded tool.

**Read [`README.md`](README.md) first.** Some tasks here touch the **locked
exit-code contract** — that is deliberate and called out. Do not touch it in any
task that does not say so.

### Dependency graph for this phase

```
3.5  commit Cargo.lock         (independent, config)
3.6  SHA-pin Actions           (independent, config)
3.7  cargo-deny + MSRV + CI    depends on 3.5, 3.6 (all edit CI)
3.8  pin contract arms         (independent, tests-only)

3.1  claude-code → PreToolUse  (independent, adapter+docs)
3.2  exit code 4 (untrusted)   depends on 1.6 (doctor rows exist)
        3.2 sub-order: CLI change → docs → each adapter
3.3  hash run: targets         (independent; coordinate w/ trust.rs tasks 2.2/2.3)
3.4  scrub check env           depends on 2.1 (same file, gate.rs)
3.9  adapter contract tests    depends on 1.5, 3.1, 3.2 (adapter behavior settled)
3.10 non-UTF8 diff batch       (independent)
```

Do 3.5 and 3.6 before 3.7 (all edit CI). Do 3.1/3.2 before 3.9. Do 2.1 before 3.4.

---

## Task 3.1 — Migrate the Claude Code adapter to `PreToolUse` (block before write)  [Finding E1]

- **Severity:** Major · **Effort:** M · **Depends on:** none (but do Task 1.7
  first if not done — the doc fix)
- **Files:** `adapters/claude-code/hooks/hooks.json`,
  `adapters/claude-code/hooks/hook.sh`,
  `crates/ironlint-core/src/adapter/registry.rs` (the claude-code registration),
  `crates/ironlint-cli/src/commands/init/*` (whatever writes the settings hook
  entry), `adapters/claude-code/README.md`, and any init/doctor tests that assert
  the claude-code event name.

### What's wrong
The Claude Code adapter uses **PostToolUse** — it fires after the edit is on disk
and can only scold the model; it does not prevent the write. The tool's own
zcode design proves the pre-write pattern works for the identical Claude-Code
payload shape (`PreToolUse`, `permissionDecision: deny`, nonzero = block). Move
the flagship adapter to pre-write so a block actually stops the edit.

### Step 1 — Study the working pre-write pattern
Read `specs/2026-07-02-zcode-adapter-design.md` (§D2, §D3) and the **reasonix**
adapter (`adapters/reasonix/hooks/hook.sh`) — reasonix already gates PreToolUse
and pipes proposed content via `ironlint check --file <path> --content -`. For
Claude Code's `PreToolUse`, the stdin payload has `tool_name` and `tool_input`
(with `content` for Write, or `old_string`+`new_string` for Edit). You have the
proposed content **before** the write, so you no longer need `synthesize_diff.sh`
or the `--diff` path — use `--content -` (this also fixes perf findings P1/P2).

### Step 2 — The changes
1. **`hooks.json`:** change the event from `PostToolUse` to `PreToolUse` (keep
   the `Edit|Write` matcher). Confirm the exact Claude Code `PreToolUse` schema
   for blocking — it expects a JSON decision object or an exit code; per the
   zcode spec, nonzero exit = block for PreToolUse, and a richer response can set
   `permissionDecision: "deny"` with a `permissionDecisionReason`. Emit the
   check's message as the reason so the model sees *why*.
2. **`hook.sh`:** for a Write, read `tool_input.content`; for an Edit, apply
   `old_string`→`new_string` to the current on-disk file to produce the proposed
   content (mirror how reasonix's `edit_file` path builds it — reuse that logic,
   do **not** hand-roll byte-offset surgery). Pipe the result to `ironlint check
   --file <path> --content -`. Map ironlint's exit codes to Claude Code's
   PreToolUse block/allow semantics: `0` → allow; `2` → deny with the message;
   `3` → fail-open by default, fail-closed under `IRONLINT_FAIL_CLOSED_ON_INTERNAL`
   (keep the existing var handling); `1`/`4` → per Task 3.2. **Drop
   `synthesize_diff.sh` from this path.**
3. **Registration:** update `adapter/registry.rs` (search for the claude-code
   entry / `PostToolUse`) so `init` writes a `PreToolUse` hook. Update whatever
   embeds the event name.
4. **Docs:** update `adapters/claude-code/README.md` to describe pre-write
   blocking (this supersedes the interim doc fix from Task 1.7).

### Step 3 — Tests
- Update any init/doctor test asserting `PostToolUse` for claude-code to
  `PreToolUse`.
- The real hook-behavior contract test is **Task 3.9** — but add at least a
  manual simulation now: feed a canned `PreToolUse` Write payload and an Edit
  payload into `hook.sh` with a stub `ironlint` returning exit 0 and exit 2, and
  confirm allow vs deny (scratch dir).

### Verify
- [ ] `bash -n adapters/claude-code/hooks/hook.sh` passes; `synthesize_diff.sh`
      is no longer invoked on the write path.
- [ ] `hooks.json` says `PreToolUse`.
- [ ] Init writes a PreToolUse hook (re-run `ironlint init --harness claude-code
      --dry-run` in a scratch dir and inspect the plan).
- [ ] Standard Verification (Rust side) + adapter manual simulation.

### Done when
The Claude Code adapter blocks a violating edit **before** it lands on disk, via
`--content -`, with the check's message surfaced as the denial reason.

---

## Task 3.2 — Add exit code 4 for untrusted; make all adapters surface it loudly  [Finding C3]

- **Severity:** Blocker · **Effort:** M/L · **Depends on:** Task 1.6 (doctor rows)
- **Files:** `crates/ironlint-cli/src/commands/check.rs` (anchor: the
  `trust::ensure_trusted` call ~line 32, and `fn exit_code`), `docs/reference/
  cli.md` (exit-code table), all four adapter scripts
  (`adapters/{claude-code,reasonix}/hooks/hook.sh`,
  `adapters/{pi,opencode}/src/index.ts`).

### What's wrong
An untrusted or since-edited config makes `check` exit **1** — the same code as a
parse error — and **every adapter maps exit 1 → allow**, so a teammate who pulled
a changed `.ironlint.yml` and never re-ran `ironlint trust` silently un-gates
every edit, with no signal. The root problem: adapters cannot distinguish
"untrusted" (should be surfaced loudly, arguably blocked) from "your config has a
typo" (fail-open is defensible). Fix: give trust-refusal its **own exit code, 4**
— a deliberate, documented extension of the 0/1/2/3 contract — and make every
adapter treat it loudly.

> **This is a sanctioned change to the locked exit-code contract.** It is the one
> place these plans extend it. Do it carefully and update the docs + all
> consumers together.

### Step 1 — Failing tests first
In `crates/ironlint-cli/tests/` (search `cli_e2e_trust` for the trust e2e
patterns and `blessed_store`):
- A test that runs `check` against an **untrusted** config and asserts exit **4**
  (today it is 1 — so this fails).
- A test that runs `check` against a config with a **parse error** and asserts
  exit **1** still (unchanged — guard against regressing the two into one code).

### Step 2 — CLI change
In `check::run`, where `trust::ensure_trusted(config)` is called (~line 32), the
error currently maps to exit 1. Branch it: a **trust** failure returns exit
**4**; a genuine config/parse/load failure stays exit **1**. You may need
`ensure_trusted` to return a distinguishable error (it already returns `Err` only
for the untrusted/mismatch case — parse errors come from `load` later), so
mapping *its* `Err` to 4 is straightforward. Confirm by reading the function:
`ensure_trusted` fails only on missing/mismatched trust, so `Err(_) from
ensure_trusted → exit 4`.

Keep `fn exit_code` (the verdict→code mapper) unchanged — 4 is emitted on the
trust-refusal path before the verdict, not from a verdict.

### Step 3 — Docs
Update the exit-code table in `docs/reference/cli.md`, the root `README.md`
"Exit codes" details block, and `AGENTS.md`'s exit-code contract section to add:
`4 — Untrusted config/gates (run `ironlint trust`)`. State that adapters should
surface 4 loudly and may treat it as fail-closed.

### Step 4 — Every adapter treats exit 4 loudly
For each adapter, add an exit-4 branch **before** the generic fail-open catch-all:
- **Pre-write adapters (reasonix, pi, opencode, and claude-code after Task 3.1):**
  exit 4 → **block** the tool call with a clear message: `ironlint is configured
  here but not trusted — run 'ironlint trust' to enable checks`. Blocking (not
  allowing) on untrusted for a pre-write gate is the safe default and makes the
  gap impossible to miss. (If you prefer allow-but-loud for developer ergonomics,
  gate the block behind a documented default; but the recommended default is
  block.)
- **PostToolUse (claude-code, only if 3.1 not yet done):** exit 4 → emit a loud,
  model-visible message on stderr and a non-zero hook exit so the user notices;
  it cannot block a landed write, but it must not be silent.
Keep the existing exit 2 (block) and exit 3 (internal) handling intact.

### Verify
- [ ] Both new CLI tests pass (untrusted → 4; parse error → 1).
- [ ] Each adapter has an explicit exit-4 branch (grep each for `4)` / `=== 4`).
- [ ] Manual simulation per adapter: stub `ironlint` `exit 4` → the pre-write
      adapters block with the trust message (scratch dir).
- [ ] Docs (cli.md, README, AGENTS.md) list exit 4.
- [ ] Standard Verification.

### Done when
Untrusted config yields exit 4, adapters surface it loudly (pre-write adapters
block), and no adapter silently allows a write when the config is untrusted.

---

## Task 3.3 — Hash in-repo scripts referenced by `run:` (close the RCE gap)  [Finding C6]

- **Severity:** Blocker · **Effort:** M · **Depends on:** none (coordinate with
  Tasks 2.2/2.3 if editing `trust.rs` concurrently — do those first)
- **Files:** `crates/ironlint-core/src/trust.rs` (anchor: `fn compute_hash`),
  possibly `crates/ironlint-core/src/config/parser.rs` /
  `crates/ironlint-core/src/config/types.rs` (to read each check's `run`/`steps`).

### What's wrong
The trust hash covers the config closure and files under `.ironlint/gates/` — but
**not** other in-repo scripts a check shells out to. A check `run: "bash
scripts/lint.sh"` is trusted by its command *string* only; `scripts/lint.sh` is
never hashed. Bless the benign check, then rewrite that script, and it runs on
the next agent edit with **no re-bless** — proven end-to-end RCE (including secret
exfil, amplified by Task 3.4). This is the load-bearing gap for the whole trust
model and the planned govern product.

### Step 1 — Decide the boundary (already decided for you)
Two acceptable strategies — implement **Strategy A** (hash them):
- **Strategy A (recommended):** at hash time, scan each check's `run` and every
  `steps[].run` for tokens that name an **in-repo file** (a path that resolves,
  relative to the project root, to an existing regular file inside the repo, e.g.
  `scripts/lint.sh`, `./tools/scan.py`). Fold each such file's bytes into the
  trust hash (same length-prefixed framing the gates files use). Editing the
  script then revokes trust until re-bless.
- (Strategy B, *not* this task: deny out-of-gates references. Harder UX; skip.)

### Step 2 — Failing test first
In `trust.rs` `mod tests`:
- Create a repo with a config whose check is `run: "bash scripts/lint.sh"` and a
  `scripts/lint.sh` file. Compute the hash. Mutate `scripts/lint.sh`. Recompute.
  Assert the two hashes **differ** (today they are identical — so this fails).
- Add a control: mutating an unrelated file (`README`) does **not** change the
  hash.

### Step 3 — The fix
1. In `compute_hash`, after folding the config closure and gates files, iterate
   the resolved config's checks. For each `run` string (and each `steps[].run`),
   extract candidate file references. A simple, robust extractor: split the
   command on whitespace and shell metacharacters, then for each token, resolve
   it against the project root; if it is an existing **regular file inside the
   repo** (canonicalize and confirm it is under the root — reuse the containment
   check that `runner.rs`/tmpfile materialization already uses), include it.
2. Read each such file with the **same symlink/regular-file guards** as Task 2.3
   (do not follow symlinks; regular files only).
3. Fold them into the hash deterministically: **sort by repo-relative path** and
   use the existing length-prefixed, identity-labeled framing so the hash is
   stable and collision-resistant. Do not double-count files already under
   `.ironlint/gates/`.
4. Keep the extractor conservative: false *positives* (hashing a file that is
   just a coincidental token) are harmless (they only add to the hash); false
   *negatives* (missing a real script) are the danger — err toward inclusion for
   anything that resolves to a real in-repo regular file.

Keep `compute_hash` under complexity 15 — extract `fn referenced_repo_files(cfg,
root) -> Vec<PathBuf>`.

### Step 4 — Surface it in `ironlint trust`
This dovetails with Task 5's S3 (make `trust` show what it blesses). At minimum,
when a check references an out-of-gates in-repo script, `trust` should be able to
list it. Not required to complete this task, but note it.

### Verify
- [ ] The new test passes: editing a referenced script changes the hash; editing
      an unrelated file does not.
- [ ] Existing trust tests still pass (a config with no external scripts hashes
      the same as before — do not change the hash for the common case; only add
      to it when scripts are referenced).
- [ ] Re-bless flow works: after editing a referenced script, `ironlint check`
      exits 4 (untrusted) until `ironlint trust` is re-run. Manually verify in a
      scratch repo.
- [ ] Standard Verification.

### Done when
Editing any in-repo script a check invokes revokes trust until re-bless — the
post-bless script-swap RCE is closed.

---

## Task 3.4 — Run checks with a scrubbed environment (no secret inheritance)  [Finding S1]

- **Severity:** Major · **Effort:** M · **Depends on:** Task 2.1 (same file,
  `gate.rs` — do the process-group change first to avoid conflicts)
- **Files:** `crates/ironlint-core/src/engine/gate.rs` (anchor: `Command::new
  ("sh")` and the `.env(...)` calls that set `IRONLINT_*`)

### What's wrong
Check subprocesses inherit the **full parent environment** — no `env_clear()`.
Any blessed check (or a swapped script per C6) reads `ANTHROPIC_API_KEY`,
`GITHUB_TOKEN`, `AWS_*`, etc. This is what turned C6 from "code exec" into "code
exec with the agent's full credential set." Checks should run with a minimal,
predictable environment.

### Step 1 — Failing test first
In `gate.rs` `mod tests`: set a fake secret env var in the test process
(`SECRET_TOKEN=shhh`), run a gate whose `run` echoes `$SECRET_TOKEN` to stdout
and blocks if non-empty (`[ -n "$SECRET_TOKEN" ] && exit 2 || exit 0`), and
assert the outcome is **Pass** (the secret was not visible) and/or that the
captured output does not contain `shhh`. Today the check sees the secret → this
fails.

> Caution: setting env vars in Rust tests is process-global and `unsafe` in
> recent editions. Prefer to assert via the **child's** view: have the gate's
> `run` read the var and report, and set the var only for the duration using a
> careful approach, or test the env-construction helper directly (Step 2 makes
> the allowlist a pure function you can unit-test without mutating process env).

### Step 2 — The fix
1. Before setting the `IRONLINT_*` vars, call `cmd.env_clear()`.
2. Re-add an explicit **allowlist** of vars a check legitimately needs:
   `PATH` (required to find tools), `HOME`, `LANG`/`LC_*`, `TZ`, `TMPDIR`, and
   the `IRONLINT_*` ABI vars this function already sets. Pull the allowlisted
   ones from `std::env::var_os` and re-apply. Extract this into a testable helper
   `fn build_check_env(env: &GateEnv) -> Vec<(OsString, OsString)>` so the
   allowlist logic is unit-testable without touching real process env.
3. Consider a documented opt-in for checks that genuinely need a token (e.g. an
   `execution.pass_env: ["FOO"]` config field) — **but that is optional**; do not
   build it unless it is trivial. The required change is the scrub + allowlist.

### Step 3 — Docs
Document in `docs/security/trust.md` (and the check ABI docs) that checks run
with a scrubbed environment: `$PATH` and `$IRONLINT_*` are present; secrets in
the parent environment are **not** passed to checks.

### Verify
- [ ] The env-visibility test passes (secret not seen by the check).
- [ ] Real checks that rely on `PATH` still work (run the repo's own
      `.ironlint.yml` grep checks against the built binary in a scratch bless —
      they use `grep`, which must still be found via PATH).
- [ ] Standard Verification.

### Done when
Check subprocesses see only an allowlisted environment; parent-process secrets
are not inherited, guarded by a test, and documented.

---

## Task 3.5 — Commit `Cargo.lock`; reverse the gitignore policy  [Finding S5]

- **Severity:** Major · **Effort:** S · **Depends on:** none
- **Files:** `.gitignore` (anchor: the `Cargo.lock` line ~line 3),
  `Cargo.lock` (currently untracked), `AGENTS.md` (the note that says the lock is
  gitignored), CLAUDE.md is a symlink to AGENTS.md so editing AGENTS.md covers it.
- **Type:** config (no test, but CI verification)

### What's wrong
`Cargo.lock` is gitignored, so cargo-dist resolves dependencies **fresh on the
runner** at each tag — v0.4–v0.7 binaries were each built against whatever
resolved that day and cannot be rebuilt bit-comparably. For a **trust-branded,
shipped binary**, that is a reproducibility and supply-chain gap, and users
cannot `cargo install --locked`. Standard guidance: commit the lock for binaries.

### Steps
1. Remove the `Cargo.lock` line from `.gitignore`.
2. `git add Cargo.lock` (the file already exists on disk from your builds).
3. Update `AGENTS.md`: find the "`Cargo.lock` is gitignored (workspace policy)"
   note and change it to state the lock is **committed** for reproducible release
   builds, and that CI/release use `--locked`.
4. *(CI `--locked` flag is added in Task 3.7 — cross-reference; do not edit CI
   here.)*

### Verify
- [ ] `git ls-files Cargo.lock` now lists it.
- [ ] `.gitignore` no longer ignores it.
- [ ] `cargo build --locked` succeeds against the committed lock (then clean up:
      the build is fine, no artifact to remove beyond the shared target).
- [ ] `AGENTS.md` reflects the new policy.

### Done when
`Cargo.lock` is tracked, `.gitignore` no longer excludes it, and the contributor
note documents the reversal.

---

## Task 3.6 — Pin all GitHub Actions to commit SHAs  [Finding S4]

- **Severity:** Major · **Effort:** M · **Depends on:** none
- **Files:** `.github/workflows/ci.yml`, `.github/workflows/release.yml`
- **Type:** config (no unit test; verified by inspection + a CI run)

### What's wrong
Every `uses:` in both workflows is a **floating tag** (`actions/checkout@v6`,
`Swatinem/rust-cache@v2`, `taiki-e/install-action@v2`, `dtolnay/rust-toolchain@
stable`, etc.). A compromised tag (cf. tj-actions/changed-files, 2025) runs
attacker code inside the release job — which holds `contents: write` — poisoning
the artifacts every `ironlint update` pulls. Pin each action to a full commit
SHA. Also scope `permissions` per-job.

### Steps
1. **Use the `pin-deps-to-sha` skill** if available (it exists in this
   environment) — it automates pinning third-party refs to immutable SHAs.
   Otherwise, for each `uses: owner/repo@vX`, look up the commit SHA that the
   tag currently points to (`gh api repos/OWNER/REPO/git/refs/tags/vX` or the
   Action's releases) and replace with `owner/repo@<40-char-sha>  # vX` (keep the
   tag in a trailing comment for readability).
2. Pin **every** `uses:` in both `ci.yml` and `release.yml`, including
   `dtolnay/rust-toolchain@stable` (pin to a SHA; note this freezes the toolchain
   channel — pair with the MSRV/toolchain decision in Task 3.7).
3. **Scope permissions:** `release.yml` grants `contents: write` at the workflow
   top level to all jobs. Move it to only the job(s) that actually create the
   release; give other jobs `contents: read`.
4. For cargo-dist-generated workflow sections, pin what you can without breaking
   `dist`'s regeneration — note in a comment which blocks are dist-managed so a
   future `dist generate` does not silently unpin them (dist has its own
   pinning options; check `dist`'s config for `github-action-commit` / SHA
   pinning support and prefer that if present).

### Verify
- [ ] `grep -nE 'uses:.*@v[0-9]' .github/workflows/*.yml` returns nothing (no
      floating version tags remain), or only dist-managed lines you have flagged.
- [ ] Each `uses:` is `@<sha>`.
- [ ] `permissions:` is scoped per-job in release.yml.
- [ ] Push to a branch and confirm CI still passes with the pinned SHAs.

### Done when
Every GitHub Action is pinned to a commit SHA and write permissions are scoped to
the jobs that need them.

---

## Task 3.7 — Add `cargo-deny`, an MSRV, and a scheduled CI run  [Finding S6, + C2 CI leg]

- **Severity:** Major · **Effort:** M · **Depends on:** Tasks 3.5, 3.6 (all edit CI)
- **Files:** `.github/workflows/ci.yml`, new `deny.toml`, `Cargo.toml`
  (`workspace.package.rust-version`), optional `rust-toolchain.toml`.
- **Type:** config

### What's wrong
No dependency-audit gate (`cargo-audit`/`cargo-deny`), no declared MSRV
(`rust-version`), no scheduled CI, and (from C2) no Windows CI leg. A known-vuln
transitive crate ships undetected; `cargo install` breakage is user-discovered; a
new stable clippy lint (CI uses `-D warnings` on floating `@stable`) breaks CI
only on the next unrelated push.

### Steps
1. **cargo-deny:** add a `deny.toml` at the repo root (run `cargo deny init` for
   a starting template if `cargo-deny` is installed, else write a minimal one
   with `[advisories]`, `[licenses]` allow-list including `Apache-2.0`/`MIT`/etc.,
   and `[bans]`). Add a CI job that installs and runs `cargo deny check advisories
   licenses bans` (use the pinned `taiki-e/install-action` or `EmbarkStudios/
   cargo-deny-action` — pinned to SHA per 3.6).
2. **MSRV:** set `rust-version = "1.<N>"` in `[workspace.package]` in the root
   `Cargo.toml` (pick a version you actually build with — check with `cargo
   msrv` if available, else a recent stable like the one that has
   `process_group`, ≥1.64; be conservative). Add a CI leg that builds/tests on
   exactly that toolchain to enforce it.
3. **Scheduled run:** add `on: schedule: - cron: '0 6 * * 1'` (weekly) to `ci.yml`
   so advisories and new-lint breakage are caught proactively, not on the next
   push.
4. **Windows CI leg (closes the C2 gap):** add `windows-latest` to the test
   matrix. Because the execution model needs a POSIX shell, either (a) install
   Git Bash / use the `shell: bash` that GitHub's Windows runners provide and run
   a smoke test that a check actually executes, or (b) if full Windows execution
   is not ready, at minimum build + run the **non-execution** unit tests on
   Windows and run the Task 1.4 `#[cfg(windows)]` fail-loud test. Document which.
5. **`--locked`:** add `--locked` to the `cargo build`/`cargo test` invocations
   now that Task 3.5 committed the lock.

### Verify
- [ ] `cargo deny check` passes locally (fix or `allow` any flagged advisory/
      license with a documented reason).
- [ ] `Cargo.toml` declares `rust-version`; a CI leg builds on it.
- [ ] `ci.yml` has a weekly `schedule` trigger and a `windows-latest` leg.
- [ ] CI is green on a branch push.

### Done when
CI audits dependencies, enforces an MSRV, runs on a schedule, and exercises
Windows — all with SHA-pinned, `--locked` steps.

---

## Task 3.8 — Pin the five unpinned contract arms with tests  [Finding T1]

- **Severity:** Major · **Effort:** S · **Depends on:** none
- **Files:** `crates/ironlint-core/src/engine/gate.rs` (tests),
  `crates/ironlint-core/src/runner.rs` (extract + test the timeout resolver),
  `crates/ironlint-cli/tests/` (a JSON-shape e2e).
- **Type:** tests-only (no behavior change) — so no "failing test first"; instead
  write tests that **currently pass** to lock the behavior, EXCEPT where they
  reveal a bug (then file it).

### What's wrong
Five locked contract elements have **no test that would fail if they regressed**:
exit `126` → Internal, real signal-death → `Internal(Signal)`, the
`IRONLINT_TIMEOUT` override + `≥1` clamp, the `$IRONLINT_EVENT` value a check
actually sees, and the **full verdict-JSON wire shape** (only the version const
and one key are pinned). The ≥80% coverage gate passed CI while these `classify()`
arms sat uncovered — proof the gate cannot see specific contract arms.

### Steps — add these tests
1. **exit 126 → Internal(NotExecutable):** in `gate.rs` tests, run a gate whose
   `run` is a non-executable file (create a file, `chmod -x`, invoke it directly
   so the shell returns 126) and assert the outcome classifies as Internal with
   the NotExecutable reason. (Find `fn command_not_found_is_internal` for the
   127 analog and mirror it.)
2. **Signal death → Internal(Signal):** `#[cfg(unix)]`, run `run: "kill -TERM
   $$"` and assert `Internal(Signal(15))` (or the reason your enum uses).
   Distinguish from the existing exit-137 test (which is a *normal* exit ≥128).
3. **`IRONLINT_TIMEOUT` override + clamp:** refactor `resolve_timeout` (runner.rs
   ~line 211) if needed so it takes the env value as a parameter (pure function),
   then unit-test: `"5"` → 5s; `"0"` → clamped to ≥1s; unset → config/default.
4. **`$IRONLINT_EVENT` value:** add a gate test whose `run` asserts
   `[ "$IRONLINT_EVENT" = write ] || exit 2` for a write, and the pre-commit
   analog, so the value the check sees is pinned (not just the clap arg).
5. **Verdict JSON wire shape:** add a CLI e2e (or a `gate.rs`/`verdict.rs` unit
   test) that serializes a full `Verdict` (with a block, an error, and passed
   entries) and asserts the **exact** JSON — top-level keys (`schema_version`,
   `ironlint_version`, `status`, `blocks`, `errors`, `passed`, `elapsed_ms`), the
   `Status` string casing, and `schema_version == 5`. Prefer an `insta` snapshot
   (the crate already depends on `insta`) so future shape changes surface as a
   reviewable diff.

If any of these reveals an actual bug (e.g. 126 is misclassified), **that** turns
into a failing-test-first fix — note it and fix it.

### Verify
- [ ] All five new tests exist and pass.
- [ ] `cargo insta review` shows the intended JSON snapshot (accept it).
- [ ] Standard Verification.

### Done when
Each of the five contract arms has a test that would fail if the behavior
regressed.

---

## Task 3.9 — Real contract tests for the claude-code + reasonix hooks  [Finding T2]

- **Severity:** Major · **Effort:** M · **Depends on:** Tasks 1.5, 3.1, 3.2
  (adapter behavior must be finalized first)
- **Files:** new tests (a `bats` file or a Rust `assert_cmd`-style harness) under
  a sensible home (e.g. `adapters/claude-code/tests/` and
  `adapters/reasonix/tests/`, or extend `crates/ironlint-cli/tests/`); fix the
  false comment in `.github/workflows/ci.yml` (~lines 54–56).

### What's wrong
The claude-code and reasonix hooks have **zero executable tests** — only
install-path assertions exist. Their exit-code translation *is* the adapter
contract. Worse, `ci.yml` **claims** these "are Rust integration tests … they
spawn the adapter hook.sh" — no test spawns any hook, giving false confidence.
(opencode and pi have real suites; match that bar.)

### Steps
1. Write a contract test per hook that:
   - Puts a **stub `ironlint`** on `PATH` (a tiny script that exits with a code
     chosen per case: 0, 2, 3, 4, and — for claude-code — that also emits a canned
     verdict JSON on stdout).
   - Feeds a **canned harness payload** on stdin (a real PreToolUse Write payload,
     an Edit payload, and a malformed-JSON payload).
   - Asserts the hook's **exit code and the block/allow decision** for each: 0 →
     allow; 2 → block with the message surfaced; 3 → fail-open by default and
     fail-closed under `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`; 4 → block with the
     trust message (per Task 3.2); malformed → the explicit guard behavior (not a
     raw `set -e` die).
   - Run everything in a temp dir; never touch the real trust store or `$HOME`.
2. Wire these into CI (a job that runs `bats` or the Rust harness). If you use
   `bats`, install it via the pinned install-action.
3. **Fix the false `ci.yml` comment** to describe what the tests actually do.

### Verify
- [ ] Running the new tests exercises `hook.sh` end-to-end for claude-code and
      reasonix across exit 0/2/3/4 and malformed input.
- [ ] The `ci.yml` comment is now accurate.
- [ ] CI runs the new job and is green.

### Done when
Both shell hooks have executable contract tests in CI covering every exit-code
branch, and the misleading CI comment is corrected.

---

## Task 3.10 — One unreadable file must not abort a whole `--diff` batch  [Finding R4]

- **Severity:** Major · **Effort:** M · **Depends on:** none
- **Files:** `crates/ironlint-cli/src/commands/check.rs` (anchor: `fn run_diff`
  ~line 101 and the `std::fs::read_to_string(&f.path)` loop ~line 137),
  possibly `crates/ironlint-core/src/runner.rs` (`CheckInput::File { content:
  String }` ~line 71).

### What's wrong
In `run_diff`, the first file whose content is not valid UTF-8 (or is unreadable)
returns **exit 1** for the whole batch — *after* earlier files' verdicts were
computed but *before* any is emitted. So a real violation in a sibling file is
silently lost, and the tier is misclassified (config error, not Block). Real
repos contain images, fixtures, UTF-16 files.

### Step 1 — Failing test first
Add a CLI e2e: a diff naming two files — one text file with a real violation
(e.g. contains `TODO` under a `no-todo` check) and one file that is invalid UTF-8
— and assert the exit code reflects the **Block** (2) from the good file, with a
`skipped: non_utf8` (or similar) note for the bad file, **not** a blanket exit 1
that hides the violation. Today it exits 1 and drops the block — so this fails.

### Step 2 — The fix
1. In the `run_diff` loop, do not `return Ok(1)` on a single file's read failure.
   Instead, **classify per file**: on a UTF-8/read error, record that file as
   skipped (with a reason) and continue to the next file. Accumulate verdicts
   across all readable files.
2. Represent the skip honestly in the output: add a `skipped` notion (an
   `ExplainOutcome::Skipped { reason: "non_utf8" }` already exists in the engine
   per the review — reuse that vocabulary) and print a stderr warning naming the
   file. Do not let a skipped file flip a Block into a pass or vice-versa.
3. Emit the aggregate verdict as usual; exit code follows the normal rules
   (Block if any file blocked, etc.). A file that could not be read is a skip,
   not a config error.

> Deeper fix (carry `Vec<u8>` content through the engine so non-UTF8 files can
> actually be *checked* rather than skipped) is larger; **skipping with a loud
> note is sufficient for this task**. If you want to do the `Vec<u8>` refactor,
> scope it separately — it touches `CheckInput` and the ABI stdin plumbing.

### Verify
- [ ] The new test passes: the good file's Block is reported; the bad file is a
      noted skip; exit code is 2, not 1.
- [ ] A diff of only-good files still behaves exactly as before.
- [ ] Standard Verification.

### Done when
An unreadable/non-UTF8 file in a `--diff` batch is skipped with a visible note
and never suppresses another file's verdict or misclassifies the exit code.
