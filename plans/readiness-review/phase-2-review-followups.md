# Phase 2 follow-ups — post-review fixes

**Provenance:** these tasks come from the adversarial code review of the phase-2
work itself (commits `07294db..54018d6`, reviewed 2026-07-02 — 10 finder angles,
per-candidate verification, gap sweep). Every task below survived verification:
**CONFIRMED** means the mechanism was proven (several empirically); **PLAUSIBLE**
means the mechanism is real but the trigger needs unusual state. Three review
candidates were REFUTED and are recorded at the bottom so nobody re-litigates
them.

**Read [`README.md`](README.md) first.** Every code task ends with Standard
Verification green and starts with a failing test.

### Dependency graph for this phase

```
2.8   opencode spawn hardening   (independent)  — coordinate with 3.2 (same code block)
2.9   gate normal-path hang      (independent)  → also closes the f65d68c test-gap
2.10  Ctrl-C group forwarding    depends on 2.9 (same file; do 2.9 first)
2.11  trust store wipe guard     (independent)
2.12  config-file read guard     (independent)
2.13  gates-dir tmpfile ↔ trust  (independent)
2.14  execution field-level merge (independent)
2.15  sweep age vs timeout clamp (independent)
2.16  trust.json.tmp orphan sweep (independent)
2.17  duplicate-key guard robustness (independent)
2.18  move load-sweep out of load() (independent)
2.19  opencode edit-path honesty  (independent; trivially after 2.8)
```

Severity order ≈ task order: 2.8 and 2.9 are the two blockers — do them first.

---

## Task 2.8 — OpenCode adapter: harden the spawn/exit translation  [review #1, #3, #7 — CONFIRMED ×3]

- **Severity:** Blocker · **Effort:** M · **Depends on:** none — but Phase 3
  Task 3.2 edits the same exit-code block; whoever lands second rebases.
- **Files:** `adapters/opencode/src/index.ts`
  (anchors: `Bun.spawnSync`, `result.exitCode === 2`,
  `IRONLINT_FAIL_CLOSED_ON_INTERNAL`), `adapters/opencode/tests/plugin.test.ts`

### What's wrong (three defects, one code block)

1. **Missing binary fail-closes.** `Bun.spawnSync` **throws** (`Executable not
   found in $PATH`, verified on Bun 1.3.8) when `ironlint` is absent. There is
   no try/catch, and a throw escaping `tool.execute.before` is exactly how this
   adapter signals BLOCK — so a missing binary hard-blocks every edit/write.
   The old `$\`...\`.nothrow()` returned exit 127 and hit log-and-allow. The pi
   adapter maps spawn failure to fail-open; opencode must match.
2. **Signal death bypasses fail-closed.** When the CLI dies by signal,
   `SyncSubprocess.exitCode` is `null` (verified on Bun 1.3.8). `null` skips
   both the `=== 2` and `=== 3` branches and lands in the generic `!== 0`
   log-and-allow arm — silently defeating `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`
   exactly when the engine is being OOM-killed / hook-timeout-killed.
3. **Event-loop freeze.** `Bun.spawnSync` is synchronous inside an async hook:
   it blocks opencode's entire event loop (all sessions, UI, timers, other
   hooks) for the full check duration — up to `timeout_secs × N checks`, or
   forever if ironlint wedges (see Task 2.9). The old `await $` kept the loop
   live.

### Step 1 — Failing tests first
In `adapters/opencode/tests/plugin.test.ts` (match the existing fakeCtx/fixture
style):
1. **Missing binary → allow.** Run the before-hook with `PATH` scrubbed so
   `ironlint` cannot resolve (e.g. spawn-affecting env or a ctx whose PATH
   points at an empty dir — mirror however the suite controls env). Assert the
   hook **resolves** (does not throw) and logs an internal-error line to
   console.error. Today this test throws.
2. **exitCode null → honors fail-closed.** Simulate a signal-killed CLI: point
   the invocation at a stub `ironlint` script that `kill -KILL $$`s itself.
   With `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`, assert the hook **throws**
   (blocks). Today it resolves (allows).

### Step 2 — The fix
1. Switch to async `Bun.spawn` and await it — the hook is already async:
   ```ts
   let exitCode: number | null
   let stdout = "", stderr = ""
   try {
     const proc = Bun.spawn([...same argv...], {
       stdin: new TextEncoder().encode(proposed),
       stdout: "pipe",
       stderr: "pipe",
     })
     ;[stdout, stderr, exitCode] = await Promise.all([
       new Response(proc.stdout).text(),
       new Response(proc.stderr).text(),
       proc.exited,
     ])
   } catch (err) {
     // Spawn failure (binary missing, EACCES): internal-error tier, NOT a block.
     exitCode = 3
     stderr = (err as Error).message
   }
   ```
   (`Bun.spawn` also throws synchronously on ENOENT — the try/catch is
   load-bearing either way.)
2. Normalize signal death into the internal tier **before** the branch chain:
   `if (exitCode === null) exitCode = 3`. Then the existing `=== 3` arm —
   including the `IRONLINT_FAIL_CLOSED_ON_INTERNAL` opt-in — handles both.
3. Keep the exit-code branch bodies otherwise as-is (Task 3.2 will rework
   them); only the "how we obtain exitCode/stdout/stderr" part changes.

### Verify
- [ ] Both new tests pass; the full `bun test` suite passes.
- [ ] Grep: no `spawnSync` remains in the adapter.
- [ ] Manually: rename the ironlint binary away, run the before-hook fixture —
      edit is allowed with a stderr log, not blocked.

### Done when
A missing binary or signal-killed CLI routes through the internal-error tier
(fail-open by default, fail-closed under the opt-in), and no gated write ever
blocks the harness event loop.

---

## Task 2.9 — `run_gate`: the normal path must not hang past the timeout  [review #2 — CONFIRMED empirically; folds in review #15]

- **Severity:** Blocker · **Effort:** M · **Depends on:** none
- **Files:** `crates/ironlint-core/src/engine/gate.rs`
  (anchors: `out_handle.join()`, `kill_process_group(&child)`,
  `wait_timeout`)

### What's wrong
After `wait_timeout` returns `Ok(Some(status))` (check exited in time), the
drain-thread joins block with **no deadline**: `read_to_end` returns only at
pipe EOF, and a backgrounded descendant (`run: "my-watcher & exit 0"`) inherits
the pipe write-ends. Reproduced empirically: `sleep 6 & exit 0` with a 1s
timeout returned `Pass` after **6.02s**; a non-exiting descendant hangs every
write hook forever. The phase-2 commits fixed this hang class only on the
timeout branch (and `killpg` runs only there). Additionally, commit `f65d68c`
(the timeout-branch drain fix) shipped with **zero tests** — a repo-rule
violation ("bug fixes start with a failing test") this task also closes.

### Step 1 — Failing test first
In `gate.rs`'s `mod tests`, `#[cfg(unix)]` (copy the
`timeout_kills_whole_process_group` harness):
- `run: "sleep 30 & exit 0"` with a ~1s timeout. Assert the outcome is `Pass`
  **and** the elapsed wall-clock is well under `sleep 30` (e.g. `< 5s`). Today
  this test takes 30s — it fails on the elapsed assertion.

### Step 2 — The fix
After reaping a normal exit status, kill the (already finished) child's process
group **before** joining the drains — the group leader is dead but surviving
members still hold the pipes:
1. Where the code currently falls through from `Ok(Some(status))` toward the
   joins, insert the same `kill_process_group(&child)` / non-unix fallback used
   on the timeout branch. Killing the group of an exited-but-unreaped leader is
   safe: the zombie keeps the pgid reserved. (Note the borrow order: call it
   before `child.wait()`-adjacent moves if any; mirror the timeout branch.)
2. Then join the drains as today — EOF is now guaranteed because every group
   member holding a write-end is dead.
3. Semantics note for the docstring: this changes behavior for checks that
   deliberately background daemons and exit 0 — the daemon no longer outlives
   the check. That is the *intended* contract (a gate must not leave residue);
   say so explicitly in the comment. A descendant that escaped the group via
   `setsid` still evades this — keep the existing timeout-branch comment's
   caveat, and bound the joins if you want belt-and-suspenders (e.g. join with
   a helper thread + channel and a `timeout`-sized deadline, falling back to
   detach like the timeout branch). The group-kill is the required part.
4. While here, add the missing `f65d68c` regression test: a check whose
   backgrounded descendant `setsid`s away while inheriting stdout, under a
   short timeout — assert `run_gate` returns `Internal(Timeout)` within a
   bounded window (this pins the detach-don't-join behavior of the timeout
   branch).

### Verify
- [ ] New tests pass; `timeout_is_internal` and `timeout_kills_whole_process_group` still pass.
- [ ] Elapsed-time assertion is generous enough not to flake (assert `< 5s`,
      not `< 1.5s`).
- [ ] Standard Verification.

### Done when
A passing check that leaves descendants cannot hold `run_gate` past its
timeout, and both drain-behavior fixes have regression coverage.

---

## Task 2.10 — Forward Ctrl-C/SIGTERM to running check groups  [review #4 — CONFIRMED]

- **Severity:** Major · **Effort:** M · **Depends on:** Task 2.9 (same file — land it first)
- **Files:** `crates/ironlint-core/src/engine/gate.rs`, possibly
  `crates/ironlint-cli/src/main.rs` (anchors: `process_group(0)`, `killpg`)

### What's wrong
`process_group(0)` removes every check from the terminal's foreground process
group, so Ctrl-C SIGINT reaches ironlint but **not** the running check. ironlint
dies; the check survives orphaned with no supervisor and no wall-clock cap
(its only enforcer was ironlint's in-process `wait_timeout`). Pre-phase-2 the
child shared the foreground group and died with ironlint. No signal handler
exists anywhere in the workspace (verified by grep).

### Step 1 — Failing test first
`#[cfg(unix)]` integration-style test: spawn the compiled `ironlint check`
binary (assert_cmd style, in ironlint-cli's e2e suite) against a config whose
check backgrounds a marker-writing sleeper, send the ironlint process SIGINT
mid-check, then assert the check's grandchild pid is dead (same
`kill(pid, 0) == ESRCH` polling pattern as `timeout_kills_whole_process_group`).
Today the grandchild survives.

### Step 2 — The fix
Install a minimal SIGINT/SIGTERM handler (nix is already a dependency with the
`signal` feature) that killpg's the currently-running check group(s) and
re-raises the default disposition:
1. Track the live child pgid in a shared slot (e.g. a `static AtomicI32`
   registered by `run_gate` around spawn/reap — 0 when idle).
2. Handler: read the slot; if nonzero, `killpg(pgid, SIGKILL)`; then restore
   the default handler and re-raise so ironlint still dies with the right
   status. Keep the handler async-signal-safe: `killpg`/`sigaction`/`raise`
   only — no allocation, no locks, no `eprintln!`.
3. Register once at CLI startup (`main.rs`) or lazily at first gate spawn —
   pick one and document it. Do **not** pull in a heavy signal crate.

### Verify
- [ ] New SIGINT test passes; existing gate tests pass.
- [ ] `ironlint check` interrupted at a terminal leaves no orphaned check
      (manual: run a `sleep 30` check, Ctrl-C, `pgrep sleep` is empty).
- [ ] Standard Verification.

### Done when
Interrupting ironlint kills the in-flight check group; nothing survives an
interactive Ctrl-C.

---

## Task 2.11 — Trust store: never silently destroy entries on parse failure  [review #5 — CONFIRMED]

- **Severity:** Major · **Effort:** S · **Depends on:** none
- **Files:** `crates/ironlint-core/src/trust.rs`
  (anchors: `classify_store_read`, `TRUST_STORE_VERSION`, `store.version =`)

### What's wrong
`classify_store_read` treats **any** serde_json parse failure as corruption →
empty store, and `bless_in` then rewrites the machine-wide store with a single
entry — destroying every other repo's blessed entry behind a stderr-only
warning. The likeliest real parse failure is not byte corruption (atomic
temp+rename makes half-writes near-impossible) but **version skew**: a store
written by a newer ironlint with a breaking shape change. The `version` field
is written but **never read back**, so an older binary can't even detect that
case.

### Step 1 — Failing tests first
In `trust.rs` `mod tests`:
1. Write a syntactically-valid store whose `version` is
   `TRUST_STORE_VERSION + 1` and whose `entries` use a shape this binary can't
   parse (e.g. `entries: 42`). Call `bless_in` and assert it returns **Err**
   mentioning a newer version — not a silent rewrite. Today it rewrites.
2. Write actual garbage (`"{ not json"`), call `bless_in`, assert it still
   succeeds (recovery path preserved) **and** that a sibling backup file
   (`trust.json.corrupt-*`) now exists containing the original bytes.

### Step 2 — The fix
In `classify_store_read` (keep it pure — pass what it needs in):
1. **Version probe before declaring corruption.** On full-parse failure, try a
   lenient parse into `serde_json::Value` and read `.version`: if it parses
   and `version > TRUST_STORE_VERSION`, return a hard `Err` ("trust store was
   written by a newer ironlint (version N); upgrade ironlint or remove the
   store") — never rewrite a future-format store.
2. **Backup on recovery.** When the content is genuinely unparseable (garbage),
   keep the recover-to-empty behavior but first copy the old bytes aside to
   `trust.json.corrupt-<pid>` (best-effort, inside the held lock), and say so
   in the warning. Recovery stays possible; data is never silently gone.
3. `ensure_trusted_in` keeps failing closed on any unreadable store — untouched.

### Verify
- [ ] Both new tests pass; `bless_recovers_from_corrupt_store` and
      `ensure_trusted_fails_closed_on_corrupt_store` still pass.
- [ ] Standard Verification.

### Done when
A future-format store is refused, a garbage store is backed up before recovery,
and no code path discards trust entries without leaving the bytes on disk.

---

## Task 2.12 — Guard config-file reads like the gates walk  [review #6 — PLAUSIBLE]

- **Severity:** Major · **Effort:** S · **Depends on:** none
- **Files:** `crates/ironlint-core/src/trust.rs` (anchors: `compute_hash`,
  `std::fs::read(` on the config path), `crates/ironlint-core/src/config/extends.rs`
  (anchor: `read_to_string`)

### What's wrong
Task 2.3 hardened the gates-dir walk against symlinks/FIFOs, but the **config
files themselves** — read by `compute_hash` and the `extends` resolver on the
same pre-trust boundary — still use plain `fs::read`/`read_to_string`.
`classify_entry`'s own docstring names the threat: a FIFO blocks the read
forever, with no timeout rail (the per-check timeout covers only gate
execution). `mkfifo .ironlint.yml` hangs every `ironlint check`, and through
the opencode adapter's synchronous invocation, the whole harness.

### Step 1 — Failing test first
`#[cfg(unix)]` in `trust.rs` tests: `mkfifo` at the config path (reuse the
`classify_entry_refuses_a_fifo` mkfifo pattern), call `compute_hash`, assert a
prompt clear `Err` mentioning "non-regular" — today it hangs (guard the test
with a thread+timeout harness so a regression fails rather than wedges CI).

### Step 2 — The fix
Route every pre-trust file read through the existing classifier: before
`fs::read`/`read_to_string` of a config path (compute_hash's config read, each
`extends` target in the resolver), call `classify_entry` and refuse anything
that is not `EntryKind::File`. Decide symlink policy deliberately: config paths
are commonly symlinked in monorepos — if you allow symlinked *configs*, resolve
via `canonicalize` first and classify the target; if you refuse, say so in the
error. Either way a FIFO/device/socket is a hard error, never a read.

### Verify
- [ ] FIFO test passes promptly; existing trust + extends tests pass.
- [ ] Standard Verification.

### Done when
No pre-trust code path can block forever on a non-regular file.

---

## Task 2.13 — Break the gates-dir tmpfile ↔ trust-hash deadlock  [review #8 — PLAUSIBLE]

- **Severity:** Major · **Effort:** S/M · **Depends on:** none
- **Files:** `crates/ironlint-core/src/trust.rs` (anchor: `collect_into`),
  `crates/ironlint-cli/src/commands/trust.rs` (anchor: `bless`),
  `crates/ironlint-core/src/runner.rs` (anchor: `TMPFILE_PREFIX`)

### What's wrong
A `$IRONLINT_TMPFILE` leaked **inside `.ironlint/gates/`** (harness kills
ironlint mid-check on a gates-dir edit) changes the gates hash → every check
fails closed **before engine load**, so neither sweep can ever run → trust is
bricked. Worse, the escape hatch loops: `ironlint trust` blesses the leak into
the hash, and >1h later the materialization-time sweep deletes the now-blessed
file → mismatch → bricked again.

### Step 1 — Failing tests first
1. In `trust.rs` tests: put an `ironlint-tmp-xxx.sh` file in a gates dir and
   assert `compute_hash` returns a **curated Err** naming the leaked tmpfile
   and the fix — not a silent hash that folds it in. (Today it hashes it.)
2. CLI-level: `ironlint trust` against that repo succeeds and the leak is
   **gone** afterward (swept during bless), with the resulting hash matching a
   leak-free dir.

### Step 2 — The fix
1. `collect_into`/`compute_hash`: on encountering a file whose name starts with
   the tmpfile prefix (export `TMPFILE_PREFIX` from runner.rs or move it to a
   shared module — do not duplicate the literal), hard-error with an actionable
   message: "leaked ironlint tmpfile in gates dir ({path}); run `ironlint
   trust` to reclaim it, or delete it". Never hash it, never skip it silently.
2. `bless_in` (before hashing): sweep stale `ironlint-tmp-*` files in each
   gates dir (reuse `sweep_stale_tmpfiles` with age 0 here — anything with the
   reserved prefix inside a gates dir is ironlint's own residue; a human file
   deliberately named with the reserved prefix is out of contract).
3. Net effect: a leak inside gates produces one loud actionable failure, and
   `ironlint trust` is a real, non-looping recovery.

### Verify
- [ ] Both tests pass; existing gates-hash tests (including nested-dir hashing)
      still produce identical hashes for leak-free dirs.
- [ ] Standard Verification.

### Done when
A gates-dir tmpfile leak cannot brick trust: `check` fails with the fix in the
message, and `ironlint trust` recovers permanently.

---

## Task 2.14 — `execution` merge: field-level, not block-level  [review #9 — CONFIRMED empirically]

- **Severity:** Major · **Effort:** S · **Depends on:** none
- **Files:** `crates/ironlint-core/src/config/types.rs` (anchors:
  `ExecutionConfig`, `timeout_secs`, `fn timeout_secs`),
  `crates/ironlint-core/src/config/extends.rs` (anchor:
  `local.execution.take()`)

### What's wrong
Empirically verified on the locked serde_yaml 0.9.34: `execution: {}` parses to
`Some(ExecutionConfig { timeout_secs: 30 })` while `execution:` (null) parses
to `None` — so through the whole-block merge, two spellings of "no explicit
settings" resolve to **opposite** timeouts (30 vs an inherited 120). A
merge-conflict leftover `execution: {}` silently reverts an org baseline's
slow-check timeout → timeouts → exit 3 → fail-open. And the moment
`ExecutionConfig` grows a second field, a child setting only that field resets
`timeout_secs` too.

### Step 1 — Failing tests first
In `crates/ironlint-core/tests/extends.rs` (copy the existing
`extends_inherits_execution_timeout_from_parent` helpers):
- Child config with literal `execution: {}\n` extending a parent with
  `timeout_secs: 120` → assert resolved `timeout_secs() == 120`. Fails today
  (resolves 30).
- Child with `execution:\n` (null) → also 120 (passes today; pins the spelling
  equivalence).

### Step 2 — The fix
Make the *field* optional, not just the block:
1. `ExecutionConfig.timeout_secs: Option<u64>` (serde default None). Remove the
   field-level `default_timeout_secs` serde default; keep the 30s fallback in
   exactly one place: `Config::timeout_secs()`.
2. Merge per-field in **both** merge functions (see also cleanup item C2 —
   deduplicating them makes this a one-line change):
   `merged.timeout_secs = local.timeout_secs.or(inherited.timeout_secs)`, with
   the block merge becoming a zip of Options rather than `.take().or(...)` on
   the whole block.
3. `Config::timeout_secs()` stays the single resolution point:
   `execution.as_ref().and_then(|e| e.timeout_secs).unwrap_or(30)`.

### Verify
- [ ] New tests pass; all existing extends/origin/timeout tests pass unchanged.
- [ ] Standard Verification.

### Done when
`execution: {}`, `execution:`, and an absent block are indistinguishable, and
future `execution` fields inherit independently by construction.

---

## Task 2.15 — Clamp the tmpfile sweep age against the resolved timeout  [review #10 — PLAUSIBLE]

- **Severity:** Minor · **Effort:** S · **Depends on:** none
- **Files:** `crates/ironlint-core/src/runner.rs`
  (anchors: `TMPFILE_SWEEP_MAX_AGE`, `resolve_timeout`, both
  `sweep_stale_tmpfiles(` call sites)

### What's wrong
The 1h sweep threshold's safety story ("a live tmpfile is always younger than
the check timeout") is documented in the const's own docstring but **not
enforced**: `execution.timeout_secs`/`IRONLINT_TIMEOUT` accept arbitrary u64.
A `timeout_secs: 7200` check 61 minutes into its run can have its live
`$IRONLINT_TMPFILE` swept by a concurrent invocation → spurious mid-run
failure.

### Step 1 — Failing test first
Unit test on a new pure helper (see fix): `effective_sweep_age(resolved_timeout)`
with `timeout = 2h` must return `≥ 4h`; with the default 30s it returns the 1h
floor. Then a call-site test: engine loaded with `timeout_secs: 7200` does not
sweep a prefix-matched file backdated 2h (it would today).

### Step 2 — The fix
`fn effective_sweep_age(timeout: Duration) -> Duration { TMPFILE_SWEEP_MAX_AGE.max(timeout * 2) }`
— use it at both sweep call sites (load-time and materialization-time; note
Task 2.18 may move the load-time one — clamp whichever survive). Update the
`TMPFILE_SWEEP_MAX_AGE` docstring: the caveat paragraph about hours-long
timeouts is now enforced, not just documented.

### Verify
- [ ] New tests pass; existing sweep tests (1h threshold with default timeout)
      pass unchanged.
- [ ] Standard Verification.

### Done when
No configuration can make the sweep collect a tmpfile that a live check may
still be using.

---

## Task 2.16 — Reclaim orphaned `trust.json.tmp.*` files  [review #11 — CONFIRMED]

- **Severity:** Minor · **Effort:** S · **Depends on:** none
- **Files:** `crates/ironlint-core/src/trust.rs`
  (anchors: `unique_tmp_path`, `bless_in`, `acquire_store_lock`)

### What's wrong
The per-write-unique temp names fixed the concurrent-clobber race but created a
leak class the old fixed name didn't have: a bless killed between `fs::write`
and `rename` orphans `trust.json.tmp.<pid>.<n>` in `~/.config/ironlint/`
forever — no sweeper covers the store directory (the runner's sweep is
repo-scoped with a different prefix).

### Step 1 — Failing test first
In `trust.rs` tests: create `trust.json.tmp.99999.0` (plus a fresh-mtime one)
next to the store path, run `bless_in`, assert the stale orphan is gone, the
fresh one survives, and the store itself is intact.

### Step 2 — The fix
Inside `bless_in`, while holding the store lock (so no live writer's temp can
be racing): best-effort remove sibling files matching
`<store-stem>.json.tmp.*` older than a generous age gate (an hour is plenty —
a live bless holds the lock for milliseconds; reuse/adapt the
`is_stale_tmpfile` age logic rather than duplicating it). Keep it a handful of
lines; failures to remove are ignored.

### Verify
- [ ] New test passes; concurrency + corruption-recovery tests still pass.
- [ ] Standard Verification.

### Done when
Crashed blesses no longer accumulate files in `~/.config/ironlint/`.

---

## Task 2.17 — Duplicate-key guard: precise message, non-degradable mechanism  [review #12 — CONFIRMED]

- **Severity:** Major · **Effort:** S · **Depends on:** none
- **Files:** `crates/ironlint-core/src/config/parser.rs`
  (anchors: `duplicate_mapping_key`, `duplicate entry with key`)

### What's wrong
Two defects in the task-2.4 guard:
1. **Misleading + lossy message.** It fires on a duplicate key *anywhere*
   (e.g. two `run:` lines inside one check body) but unconditionally says
   "each check id must be unique", and it discards serde_yaml's line/column.
   The pre-change raw error (`duplicate field \`run\`` with position) was more
   accurate for body-field duplicates.
2. **Silently degradable mechanism.** Detection string-matches serde_yaml's
   English error text. serde_yaml is archived; a migration to a fork (or any
   wording change) makes the marker miss → `duplicate_mapping_key` returns
   `None` → the `Config` parse silently succeeds **last-write-wins** — the
   guard's failure mode is the exact policy-disarming bug it was built to fix,
   with no signal.

### Step 1 — Failing tests first
1. Body-field duplicate: config with two `run:` keys in one check → assert the
   error names `run` but does **not** claim a duplicate *check id*, and that it
   preserves serde's position info (assert the message contains `line`).
2. Canary pinning the mechanism: assert
   `serde_yaml::from_str::<serde_yaml::Value>("a: 1\na: 2\n")` yields an error
   whose text contains the exact marker `duplicate entry with key` — with a
   comment explaining this test exists so a serde_yaml swap/upgrade that
   changes the wording fails **loudly here** instead of silently disarming the
   guard. (Passes today; it's the tripwire.)

### Step 2 — The fix
1. Keep the Value-parse detection but carry the **whole** serde error through:
   error text = `duplicate key \`{key}\` ({original serde message with
   line/col})`, and reword the hint to cover both cases: "if this is a check
   id, each id must be unique; if it is a field, remove the repeated line."
   (Cheap improvement: only claim check-id-ness when a quick
   `input.contains(...)` heuristic isn't needed — the generic wording is fine;
   don't over-engineer scope detection.)
2. Add a fallback tripwire in `parse_str` itself: if the `Value` parse errored
   but the marker extraction returned `None`, do **not** proceed to the
   `Config` parse silently — surface the raw serde_yaml error instead. A YAML
   document whose `Value` parse fails must never produce an `Ok(Config)`.
   (This one line removes the silent-degradation mode entirely; the canary
   test then guards message quality rather than correctness.)

### Verify
- [ ] New tests pass; the five existing duplicate-id tests pass unchanged.
- [ ] Standard Verification.

### Done when
A duplicate key error is accurate and positioned, and no serde_yaml change can
silently turn the guard off.

---

## Task 2.18 — Move the config-root sweep out of `IronLintEngine::load`  [review #13 — PLAUSIBLE]

- **Severity:** Minor · **Effort:** S · **Depends on:** none
- **Files:** `crates/ironlint-core/src/runner.rs` (anchor:
  `sweep_stale_tmpfiles(&config_dir_canon`), `crates/ironlint-cli/src/commands/check.rs`

### What's wrong
The file-deleting sweep runs inside `load()`, which the architecture leans on
as pure (trust enforcement was deliberately kept out of it for that reason).
Verified blast radius: `explain` and `watch` startup — read-only surfaces —
now delete prefix-matched files in the target directory, including for
**never-blessed** configs; and the match is prefix+mtime only, so a committed
file named `ironlint-tmp-*` checked out >1h ago is deleted by a read-only
command. `validate`/`show-resolved-config` don't load the engine and are
unaffected; `check` enforces trust before load.

### Step 1 — Failing test first
CLI e2e: in a repo with an **untrusted** config and a backdated
`ironlint-tmp-old.rs` next to `.ironlint.yml`, run `ironlint explain` — assert
the file **still exists** afterward. Today it is deleted.

### Step 2 — The fix
Remove the sweep call from `load_with` and invoke it from the `check` command
path instead (after `ensure_trusted`, before/alongside dispatch — one call per
check invocation, same as today's frequency for the write hook). The
materialization-time sweep (the one that actually reclaims nested leaks) stays
where it is. Update the `TmpFileGuard` docstring's two-backstops paragraph to
name the new location.

### Verify
- [ ] New e2e test passes; the existing load/materialization sweep tests are
      updated to drive the new entry point and pass.
- [ ] Standard Verification.

### Done when
Read-only commands never mutate the target directory; `load()` is pure again;
leak reclamation still happens on every gated write.

---

## Task 2.19 — OpenCode edit path: honest comment, visible skip  [review #14 — CONFIRMED]

- **Severity:** Minor · **Effort:** S · **Depends on:** Task 2.8 (same file)
- **Files:** `adapters/opencode/src/index.ts`
  (anchors: `travels byte-for-byte`, `can't simulate — skip the gate`,
  `computeProposedContent`)

### What's wrong
Two honesty gaps: (1) the comment added in phase 2 claims content "travels
byte-for-byte", but `computeProposedContent` simulates edits via
`readFileSync(filePath, "utf8")` — lossy for non-UTF8 files, so checks judge
U+FFFD-mangled bytes that never land on disk; (2) the
`if (proposed === null) return` skip is **completely silent** — an unmatched
`oldString` (routine on stale edits) skips the gate with zero output,
indistinguishable from a pass. Also, the non-UTF8 regression test covers only
the `write` tool.

### Step 2 — The fix (test where noted)
1. Scope the comment: byte-for-byte holds for `write`; `edit` simulation is a
   lossy UTF-8 round-trip of the current file. Two sentences.
2. Log the skip: `console.error("ironlint: cannot simulate edit for <path>
   (oldString not found); gate skipped")` before the `return`. Add a test
   asserting the log fires on an unmatched-oldString edit (assert via the same
   console.error capture the R3 tests use).
3. Optional if cheap: an `edit`-tool sibling of the non-UTF8 write test
   documenting the current lossy behavior, so a future byte-clean fix has a
   baseline.

### Verify
- [ ] `bun test` passes with the new assertion.

### Done when
The gate never skips invisibly and no comment overclaims byte fidelity.

---

## Cleanup shortlist (no dedicated tasks — batch opportunistically)

Below the severity cut but real; each is small and most reduce future drift:

- **C1 — `parse_str` triple parse:** `legacy_marker`, `duplicate_mapping_key`,
  and the `Config` deserialize each parse the same string. One
  `from_str::<Value>` result can feed all three (`Err` → duplicate extraction;
  `Ok(value)` → legacy scan; then `from_value::<Config>`). Task 2.17's
  tripwire makes the hidden ordering dependency explicit either way.
- **C2 — merge duplication:** `merge_inherited` and
  `merge_inherited_with_origin` copy the same merge semantics; implement
  `resolve` via `resolve_with_origin(..).map(|(c, _)| c)` or delegate, so
  `check` and `explain` cannot diverge. Do before/with Task 2.14.
- **C3 — atomic-write consolidation:** `write_store` re-implements
  `adapter::materialize::atomic_write` — which still has the **fixed** `.tmp`
  sibling name (the concurrent-clobber race 2.2 fixed for trust only; two
  parallel `ironlint init` runs can interleave on settings.json). Move
  unique-temp-naming into `atomic_write`, call it from both, and that fixes
  materialize's race for free.
- **C4 — opencode block message:** on exit 2 the adapter throws the entire
  pretty-printed verdict JSON; extract `blocks[].message` like pi's
  `blockReason()` does. Natural rider on Task 2.8/3.2.
- **C5 — trust walk read pattern:** `classify_entry` lstats by path then
  `fs::read`s by path (a swap window on the security boundary, plus one
  redundant lstat per file). `DirEntry::file_type()` closes the enumerate→stat
  gap for free; open-with-`O_NOFOLLOW` + `fstat` + read-from-fd closes it
  fully. Fold into Task 2.12 if convenient.
- **C6 — `kill_process_group` signatures:** make both cfg variants take
  `&mut Child` so the call site is one un-cfg'd line; drop the non-unix stub's
  dead `create_dir_all` in `acquire_store_lock` while there.
- **C7 — test robustness:** `timeout_kills_whole_process_group` races sh
  startup against a 500ms budget (raise to ~2s with a longer sleep);
  `concurrent_blesses_do_not_lose_entries` asserts serialization the non-unix
  lock stub doesn't provide — `#[cfg(unix)]`-gate it until Windows locking
  lands.

## Verified non-issues (do not re-litigate)

- **opencode cwd/relative `--file`:** the removed `$` was the raw global
  `Bun.$` (same server cwd as `spawnSync`), and `resolve_input_path` anchors
  relative paths to `config_dir` from the always-absolute `--config` — not the
  child's cwd. No behavior change, no bypass.
- **`collect_into` vanished-entry bail:** the pre-change walk hard-errored on
  the same race (`fs::read` → NotFound). Wording changed, surface didn't; the
  bail is the right security posture.
- **"checks reading `$IRONLINT_FILE` see pre-edit content":** intended ABI —
  stdin/`$IRONLINT_TMPFILE` carry proposed content; the new adapter test
  documents this deliberately.
- Also checked and fine: `check --content -` exists at the reviewed commit;
  `Bun.spawnSync` pipes stderr by default (no undefined deref on the exit-3
  path); nix 0.31.3 resolves; `skip_serializing_if` on `execution` changes no
  observable output; `wait_timeout` works on a group leader; multi-parent
  `extends` execution merge is first-parent-wins, consistent with checks.
