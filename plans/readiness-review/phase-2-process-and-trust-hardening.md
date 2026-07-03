# Phase 2 — Process & trust hardening

**Goal:** fix the correctness/robustness bugs where IronLint reaches a wrong
verdict, leaks state, or races itself. Several of these also *unblock* later
work (2.1 process-groups is a prerequisite for Phase 4's parallel dispatch; 2.5
is a prerequisite for the perf spec's `panic = "abort"`).

**Read [`README.md`](README.md) first.** Every code task ends with Standard
Verification green and starts with a failing test.

### Dependency graph for this phase

```
2.1  process-tree kill        (independent)   → unblocks 4.6 (parallel dispatch)
2.2  trust store locking      (independent)  ─┐  both touch trust.rs — do 2.2
2.3  trust walk symlink guard depends on 2.2 ─┘  first, then 2.3, to avoid conflicts
2.4  reject duplicate ids     (independent)
2.5  stale-tmpfile sweep      (independent)   → prerequisite for perf `panic=abort`
2.6  opencode no shadow-write (independent)
2.7  extends inherits execution (independent)
```

Do 2.2 before 2.3 (same file). Everything else is independent.

---

## Task 2.1 — Kill the whole process tree on timeout  [Finding C5]

- **Severity:** Blocker · **Effort:** M · **Depends on:** none
- **Files:** `crates/ironlint-core/src/engine/gate.rs`
  (anchors: `Command::new("sh")` ~line 77, `let _ = child.kill();` ~line 131,
  the `read_to_end` drain calls ~lines 119/124)

### What's wrong
On timeout, the engine kills only the `sh` child (`child.kill()`). Real checks
are compound (`cargo check 2>&1 | head`), so `sh` fork-execs the actual tool as a
child; killing `sh` leaves that tool **orphaned and still running**. An orphaned
`cargo` holds the target-dir lock, so every *subsequent* write-check also times
out → exit 3 → cascading fail-open, plus zombie processes accumulate. (Observed
live: a wedged run left a stack of orphaned `synthesize_diff.sh` processes.)

### Step 1 — Failing test first
Add a test in `gate.rs`'s `mod tests` (search for `fn timeout_is_internal` to
find the block and copy the harness for building a `GateEnv` + running a gate
with a short timeout). The test must prove the **grandchild** is killed:
- Run a gate whose `run` backgrounds a marker: something like
  `sh -c 'sleep 30 & echo $! > "$MARKER"; wait'` where `$MARKER` is a temp file
  path you pass in, with a 1-second timeout.
- After the gate returns (InternalError/timeout), read the pid from `$MARKER`
  and assert the process is **no longer alive** (`kill(pid, 0)` returns
  `ESRCH`; on Unix use `libc`/`nix` if already a dep, otherwise check via
  `Command::new("kill").args(["-0", &pid])` status is non-zero).

This test **fails today** (the grandchild survives). Keep it `#[cfg(unix)]` —
process groups are a Unix concept and this is a Unix-execution tool.

### Step 2 — The fix
1. Put the child in its **own process group** at spawn. On the `Command` builder
   (search for `Command::new("sh")`), before `.spawn()`, add:
   ```rust
   #[cfg(unix)]
   {
       use std::os::unix::process::CommandExt;
       cmd.process_group(0); // new group, leader = the sh child
   }
   ```
   (`process_group` is stable since Rust 1.64.)
2. On timeout, kill the **whole group**, not just the child. Where the code
   currently does `let _ = child.kill();`, replace with a group kill of the
   child's pid (which is the group id because it is the group leader):
   ```rust
   #[cfg(unix)]
   {
       // SAFETY: killpg on our own child's process group; pid is valid until reaped.
       let pgid = child.id() as i32;
       unsafe { libc::killpg(pgid, libc::SIGKILL); }
   }
   #[cfg(not(unix))]
   let _ = child.kill();
   ```
   If `libc` is not already a dependency of `ironlint-core`, add it
   (`libc = "0.2"` in the crate's `Cargo.toml`; it is a tiny, standard dep). The
   workspace denies `unsafe_code` by default — add a scoped
   `#[allow(unsafe_code)]` on the block **with a `// SAFETY:` comment** (the
   workspace lint config in the root `Cargo.toml` explicitly allows this pattern;
   see its comment about `clone(2)`).
   Alternatively, use the `nix` crate's safe `killpg(Pid::from_raw(pgid),
   Signal::SIGKILL)` to avoid `unsafe` entirely — **prefer `nix` if you can** so
   no `unsafe` is introduced. Check whether `nix` is already available before
   adding a new dep.
3. **Join the drain threads.** After the kill, the stdout/stderr reader threads
   (the ones doing `read_to_end`) can block on pipes held open by (now-killed)
   descendants. Ensure the code `join()`s them (or drops the child's pipe handles
   first so the reads return EOF). Confirm no thread handle is left unjoined on
   the timeout path.

### Verify
- [ ] The new `#[cfg(unix)]` test passes — the grandchild is dead after timeout.
- [ ] `fn timeout_is_internal` and the other gate tests still pass.
- [ ] No new `unsafe` without a `// SAFETY:` comment (prefer `nix` to avoid it).
- [ ] Standard Verification.

### Done when
A timed-out compound check leaves no surviving descendant process, proven by a
test.

---

## Task 2.2 — Lock the trust store; unique temp file; recover from corruption  [Finding R5]

- **Severity:** Major · **Effort:** S · **Depends on:** none
  (do **before** Task 2.3 — same file)
- **Files:** `crates/ironlint-core/src/trust.rs`
  (anchors: `fn write_store` ~line 162, `path.with_extension("json.tmp")` ~line
  168, `fn read_store` ~line 153, `fn bless_in` ~line 198)

### What's wrong
Blessing the store is an **unlocked read-modify-write** with a **fixed temp
filename** (`json.tmp`). Two concurrent `ironlint trust` runs (parallel agent
sessions across repos — the core scenario) lose entries: verified 4 parallel
blesses → 1 surviving entry. And a corrupt/half-written store makes `read_store`
error, which bricks **both** `check` (exit 1 everywhere) **and** `trust` (it
reads before writing) — only a manual `rm` recovers.

### Step 1 — Failing tests first
In `trust.rs` `mod tests`:
1. **Concurrency test:** spawn N threads (e.g. 8), each blessing a *different*
   config path into the **same** store file, `join` them all, then assert the
   store contains **N** entries. Today this fails (lost updates). Use
   `std::thread` and a shared store path in a `tempfile::tempdir()`.
2. **Corruption-recovery test:** write garbage bytes (e.g. `"{ not json"`) to the
   store path, then call `bless_in(new_config, store_path, now)` and assert it
   **succeeds** and the store afterward contains the new entry. Today this fails
   (bless reads first and errors).

### Step 2 — The fix
1. **Lock around the RMW.** `fs4` is already a workspace dependency (see root
   `Cargo.toml`). Open the store file (or a sibling `.lock` file) and take an
   exclusive `fs4` lock for the whole read-modify-write in `bless_in`. Release on
   drop. This serializes concurrent blesses.
2. **Unique temp name.** In `write_store`, replace the fixed
   `path.with_extension("json.tmp")` with a unique name per write, e.g. include
   the pid and a counter/nanos: `path.with_extension(format!("json.tmp.{}",
   std::process::id()))`, or use `tempfile::NamedTempFile::new_in(parent)` +
   `persist`. Keep the write atomic (write temp, then rename over the target).
3. **Corruption tolerance in bless.** In `bless_in` (and only there, not in
   `ensure_trusted` — an unreadable store must still fail *checks* closed), treat
   an unparseable existing store as **empty** so a re-bless can recover: wrap the
   `read_store` call so a parse error yields an empty `TrustStore` with an
   `eprintln!` warning ("trust store was unreadable; rewriting"). `ensure_trusted`
   must keep failing closed on a corrupt store (do not weaken it).

### Verify
- [ ] Both new tests pass (8 concurrent blesses → 8 entries; bless recovers from
      a corrupt store).
- [ ] `ensure_trusted` still returns `Err` for a corrupt store (add/keep a test
      — the existing `read_store_surfaces_non_notfound_errors` covers part of
      this; make sure you did not weaken the check path).
- [ ] Standard Verification.

### Done when
Concurrent blesses no longer lose entries, and a corrupt store no longer bricks
`trust` (while `check` still fails closed on corruption).

---

## Task 2.3 — Trust hash walk: don't follow symlinks or read non-regular files  [Finding R7]

- **Severity:** Major · **Effort:** S · **Depends on:** Task 2.2 (same file)
- **Files:** `crates/ironlint-core/src/trust.rs`
  (anchors: the `read_dir` walk ~line 27, `if path.is_dir()` ~line 30,
  `std::fs::read(&path)` ~lines 41/70, `gates_dir.is_dir()` ~line 89)

### What's wrong
The gates-directory hash walk runs **before** the trust verdict, on *unblessed*
repo content, and it is the security boundary — so it should be the most paranoid
code in the tree. Instead it uses `is_dir()` (which **follows symlinks**) and
`fs::read` on whatever it finds. A self-referencing symlink recurses to `ELOOP`;
a symlink to a FIFO blocks `fs::read` forever; a symlink to `/dev/zero` reads
unbounded.

### Step 1 — Failing test first
In `trust.rs` `mod tests`, `#[cfg(unix)]`:
- Create a temp `.ironlint/gates/` dir, put a normal script in it, then add a
  **symlink loop** (`std::os::unix::fs::symlink(&dir, dir.join("loop"))`).
- Call `compute_hash(config_path)` (with a config whose gates dir is this one)
  and assert it returns an **`Err` with a clear message** (not a hang, not a raw
  ELOOP). Today it recurses to a raw error / can hang on a FIFO — so this test
  fails or hangs. To be safe against a hang, you may assert the behavior of the
  new guarded walk directly.

### Step 2 — The fix
In the walk (search for `read_dir` and `is_dir()`):
1. Use `symlink_metadata` (a.k.a. `Path::symlink_metadata`, which does **not**
   follow links) instead of `is_dir()`/`is_file()` to classify each entry.
2. **Skip or hard-error on symlinks.** For a security boundary, hard-error is
   safer than silently skipping (a skipped file is un-hashed and thus not
   trust-covered). Return an `Err` like: `gates dir contains a symlink (`{path}`);
   refuse to hash — replace it with a regular file`.
3. **Only read regular files.** If an entry is not a regular file (FIFO, socket,
   device, dir-that-should-be-recursed), either recurse (real dirs, checked via
   `symlink_metadata().is_dir()`) or hard-error (non-regular). Never `fs::read` a
   non-regular file.

Keep the walk's complexity ≤15 — extract a `fn classify_entry(&Path) ->
Result<EntryKind>` helper if it gets branchy.

### Verify
- [ ] The symlink-loop test returns a clear `Err`, no hang (run with a timeout;
      `cargo test` should complete promptly).
- [ ] Normal gate files still hash identically — the existing
      `compute_hash`/trust tests still pass (do **not** change the hash of a
      normal, symlink-free gates dir).
- [ ] Standard Verification.

### Done when
The trust walk refuses symlinks and non-regular files with a clear error and
cannot hang or loop.

---

## Task 2.4 — Reject duplicate check ids at parse time  [Finding R1]

- **Severity:** Major · **Effort:** S · **Depends on:** none
- **Files:** `crates/ironlint-core/src/config/parser.rs`
  (anchors: `pub fn parse_str` ~line 10, `serde_yaml::from_str(input)` ~line 23)

### What's wrong
The config deserializes straight into a map, so **duplicate check ids are
silently accepted, last-definition-wins** — and `ironlint validate` reports
"ok." A copy-paste or a merge-conflict resolution in `.ironlint.yml` can disarm
a blocking check while the tool blesses the file. A policy tool must not silently
drop a policy.

### Step 1 — Failing test first
In `parser.rs` `mod tests` (search `fn parses_a_checks_config` to find the
block), add:
```rust
#[test]
fn duplicate_check_ids_are_rejected() {
    let err = parse_str(
        "checks:\n  dup:\n    files: \"*.rs\"\n    run: \"exit 2\"\n  dup:\n    files: \"*.rs\"\n    run: \"true\"\n"
    ).unwrap_err().to_string();
    assert!(err.contains("dup"), "error should name the duplicated id: {err}");
}
```
Run it — it **fails today** (serde_yaml silently keeps the last `dup`).

### Step 2 — The fix
`serde_yaml` does not reject duplicate mapping keys by default. Detect them
before/alongside the `Config` parse:
1. Parse the input once as `serde_yaml::Value` (the file already does this for
   legacy-key sniffing — search for `serde_yaml::Value` ~line 90 to reuse the
   pattern).
2. Navigate to the `checks:` mapping. `serde_yaml::Value`'s `Mapping` preserves
   insertion but collapses duplicates — so instead scan the **raw** duplicates:
   the robust way is to deserialize the `checks` node into a
   `Vec<(String, ...)>` via a helper, or to walk the YAML with a small check that
   counts key occurrences. Simplest reliable approach: implement a tiny
   duplicate-key detector by deserializing with `serde_yaml`'s support for
   detecting duplicates — **or** pre-scan the text for the check-id lines.

   Recommended concrete approach (deterministic, no fragile text scanning): use
   the `serde_yaml::Value` API to get the `checks` mapping, but detect duplicates
   by parsing the document with a `BTreeSet` accumulator inside a custom
   `Deserialize` visitor **is overkill**. Instead, add the crate feature or use
   `serde_yaml`'s `Value` and compare the count of `checks` child keys in the
   raw node against a de-duplicated set — if `serde_yaml` already collapsed them,
   fall back to a **line-based pre-scan**: collect top-level check-id keys (lines
   matching `^  (\S+):` within the `checks:` block) and error if any id repeats.

   > Pick ONE method and keep it simple. The line-based pre-scan of the `checks:`
   > block is acceptable and easy to get right for the 2-space-indented map this
   > tool uses. Whatever you choose, the error message must **name the duplicated
   > id** and point at the fix ("each check id must be unique").
3. Emit the error from `parse_str` before returning `Ok`, so both `validate` and
   `check` reject it.

### Verify
- [ ] The new test passes; a duplicate id is a hard error naming the id.
- [ ] A normal config with unique ids still parses (existing tests pass).
- [ ] `ironlint validate` on a duplicate-id config now exits 1, not "ok."
- [ ] Standard Verification.

### Done when
Duplicate check ids are a curated parse error; `validate` no longer blesses a
config that silently drops a check.

---

## Task 2.5 — Sweep stale temp files; fix the false cleanup comment  [Finding R6]

- **Severity:** Major · **Effort:** S · **Depends on:** none
  (this is a **prerequisite** for the perf spec's `panic = "abort"` — see note)
- **Files:** `crates/ironlint-core/src/runner.rs`
  (anchors: the comment `Only a SIGKILL of ironlint itself leaks the file` ~line
  276, `struct TmpFileGuard` ~line 277, `impl Drop for TmpFileGuard` ~line 281,
  `fn maybe_materialize_tmpfile` ~line 503)

### What's wrong
The `$IRONLINT_TMPFILE` cleanup relies on `Drop`, which does **not** run on a
default `SIGTERM`/`SIGINT` — and there is no signal handler. The comment claims
"Only a SIGKILL of ironlint itself leaks the file," which is **false**. Harnesses
kill hooks on their own timeout budgets routinely, so a real-extension sibling
(`ironlint-tmp-*.rs`) gets left in the worktree — it shows in `git status`, is
swept into agent context, and can be committed.

### Step 1 — Failing test first
In `runner.rs` `mod tests`:
- Manually create a stale temp file matching the tmpfile naming scheme (search
  `maybe_materialize_tmpfile` to see the exact prefix — it is `ironlint-tmp-`)
  in a temp "project root", with an mtime in the past (or just create it, if the
  sweep is age-gated make the test file old enough / make the age threshold
  injectable).
- Call the new sweep function (Step 2) and assert the stale file is gone while a
  same-named-but-*fresh* file (or an unrelated file) is left alone.

### Step 2 — The fix
1. **Add an age-gated sweep** that runs at engine load (search for where the
   engine is constructed / `load` / `load_with` in `runner.rs`). Implement `fn
   sweep_stale_tmpfiles(root: &Path, max_age: Duration)` that removes files in
   the project matching the `ironlint-tmp-*` prefix whose mtime is older than
   `max_age` (e.g. 1 hour). Only sweep the tmpfile naming pattern — never touch
   other files. Keep it best-effort (ignore individual removal errors) and ≤15
   complexity.
2. **Fix the false comment** at the `TmpFileGuard` docstring to state the truth:
   `Drop` handles the normal and panic-unwind paths; a `SIGTERM`/`SIGINT`/
   `SIGKILL` of ironlint mid-check leaks the file, which the load-time sweep
   (above) reclaims on the next run.
3. *(Optional, only if straightforward)* a minimal `SIGTERM`/`SIGINT` handler
   that removes known-live tmpfiles. This is optional; the sweep is the required
   part. If you add a handler, do not introduce a heavy signal crate — keep it
   minimal or skip it.

### Note for later
This sweep is what makes the perf spec's proposed `panic = "abort"` (spec item
P2) safe — with `panic = "abort"`, `TmpFileGuard`'s unwind-based cleanup would
never run, so the sweep becomes the only reclaimer. Do **not** set `panic =
"abort"` here; just know this task unblocks it.

### Verify
- [ ] The sweep test passes; stale tmpfiles are reclaimed, fresh/unrelated files
      are untouched.
- [ ] The `TmpFileGuard` comment is now accurate.
- [ ] Standard Verification.

### Done when
Stale `ironlint-tmp-*` files are reclaimed on load and the code comment tells the
truth about signal-death leaks.

---

## Task 2.6 — OpenCode adapter: stop shadow-writing the real file  [Finding E3]

- **Severity:** Major (data-loss risk) · **Effort:** S/M · **Depends on:** none
- **Files:** `adapters/opencode/src/index.ts`
  (anchors: the block that writes proposed content to the real path, runs
  `ironlint check --file`, and restores in a `finally`; search for `check --file`
  and `finally` and `readFileSync`)

### What's wrong
The OpenCode adapter writes the **proposed** content to the real file path, runs
`ironlint check --file`, then restores the original in a `finally`. This is
dangerous: (a) a **non-UTF8 file** is read back with lossy decoding and written
on restore → **permanent corruption even when the check passes**; (b) a crash
mid-check leaves blocked content on disk; (c) file watchers (HMR, `tsc --watch`)
build the flashed content. The `--content -` + `$IRONLINT_TMPFILE` machinery
exists precisely to avoid this.

### Step 1 — Understand the target shape
Read how the **pi** adapter does it — `adapters/pi/src/index.ts` calls
`spawnSync("ironlint", ["check", "--file", <path>, "--content", "-"], { input:
<proposedBytes> })` and never touches the real file. That is the model. The
opencode adapter should pipe the proposed content on **stdin** via `--content -`
and never write the real path.

### Step 2 — The fix
1. Replace the shadow-write/restore block with a single `ironlint check --file
   <path> --content -` invocation, passing the proposed content on **stdin**
   (Bun: `Bun.spawn`/`Bun.spawnSync` with `stdin` set to the bytes, or the
   `$`-shell form with piped input — match the file's existing spawn style).
2. Pass the proposed content as **bytes**, not a lossily-decoded string, so
   non-UTF8 content is not mangled. If OpenCode gives you the content as a
   string already, that is the harness's representation — but do **not** add your
   own read-back-and-rewrite of the real file. The whole shadow-write/restore
   (`cp`/`readFileSync`/`writeFileSync` of the real path) must be **deleted**.
3. Keep the block-message extraction and the fail-open/fail-closed exit handling
   exactly as they are (only the "how we feed content to ironlint" part changes).

### Verify
- [ ] The real file on disk is **never** modified by the adapter (grep the file:
      no `writeFileSync`/`cp` of the target path remains).
- [ ] `cd adapters/opencode && bun test` passes (if bun is installed). If the
      existing tests assumed the shadow-write, update them to assert the new
      stdin-piped behavior.
- [ ] Manually: pipe a proposed edit through the plugin against a fixture repo
      (scratch dir) and confirm a **pass** leaves the original file byte-identical
      (test with a non-UTF8 fixture specifically).

### Done when
The OpenCode adapter gates via `--content -` on stdin and never writes or
restores the real file; a passing check cannot corrupt a non-UTF8 file.

---

## Task 2.7 — `extends`: inherit `execution` (timeouts), not just checks  [Finding R8]

- **Severity:** Major · **Effort:** S · **Depends on:** none
- **Files:** `crates/ironlint-core/src/config/extends.rs`
  (anchors: `fn merge_inherited` ~line 78 — `local.checks.entry(id).or_insert(check)`)

### What's wrong
`merge_inherited` merges only the `checks` map — a base config's `execution`
block (e.g. `timeout_secs: 120` for slow gates) is **silently ignored** in
consuming repos, which revert to the 30s default. Those slow checks then die on
timeout → exit 3 → fail-open. The whole point of `extends` (and the planned
govern product) is org baselines, so silently dropping the base's timeout is a
real correctness bug.

### Step 1 — Failing test first
In `extends.rs` `mod tests` (search `fn extends_local_gate_wins_on_collision` to
find the block and its temp-file config-writing helpers), add:
- Write a base config with `execution:\n  timeout_secs: 120` and a check.
- Write a child config that `extends` the base and sets **no** `execution` (so it
  should inherit 120), plus its own check.
- Resolve it and assert the resolved `execution.timeout_secs == 120`. Today this
  fails (child defaults to 30).
- Add a second case: child sets `timeout_secs: 5` explicitly → resolved is `5`
  (local wins).

### Step 2 — The fix
Define merge semantics for `execution` matching the "local wins, else nearest
ancestor" rule already used for checks:
1. In `merge_inherited` (and its origin-tracking twin `merge_inherited_with_origin`
   ~line 128 — **update both**), after merging checks, merge `execution`: if the
   local config did **not** explicitly set a given `execution` field, take the
   inherited value.
2. The tricky part is distinguishing "explicitly set to the default" from "not
   set." Check how `execution`/`timeout_secs` is typed in
   `crates/ironlint-core/src/config/types.rs` (search `execution` / `timeout_secs`
   / `struct Execution`). If fields are `Option<..>` you can merge with
   `or`. If they are concrete with a serde default, you may need to make the
   field `Option` at the parse layer (resolving the default *after* merge) — do
   the minimal change that lets you tell "unset" from "set." If this requires a
   types change, keep it small and re-run the full suite.
3. Preserve local-wins: an explicitly-set child value must override the base.

### Verify
- [ ] Both new tests pass (inherit 120; local 5 wins).
- [ ] Existing `extends`/resolve tests still pass (checks still merge local-wins).
- [ ] Standard Verification.

### Done when
A base config's `execution` (timeouts) is inherited unless the child overrides
it, guarded by tests, in both merge functions.
