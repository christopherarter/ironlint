# Phase 1 — Stop the silent bypasses

**Goal:** make the gate fail *loud* instead of *silent* in the cheapest,
highest-value places. Most of these are small. None require deep architecture.

**Read [`README.md`](README.md) first** — especially "Repo rules" and "Standard
Verification." Every code task here must end with Standard Verification green.

### Dependency graph for this phase

```
1.1  LICENSE                      (independent)
1.2  init --dry-run honest        (independent)
1.3  usage errors exit 1          (independent)
1.6  doctor: trust + no-hooks row (independent)  ──┐
1.4  Windows fail-loud            depends on 1.6 ──┘ (reuses the doctor-row pattern)
1.5  reasonix fail-closed arm     (independent)
1.7  fix claude-code doc claim    (independent, doc-only)
```

Do 1.6 before 1.4. Everything else is independent.

---

## Task 1.1 — Add the missing `LICENSE` file  [Finding C1]

- **Severity:** Blocker · **Effort:** S · **Depends on:** none
- **Files:** new file `LICENSE` at repo root
- **Type:** non-code (no test required)

### What's wrong
`README.md` shows an "Apache 2.0" badge linking to `LICENSE`, and `Cargo.toml`
declares `license = "Apache-2.0"`, but **no `LICENSE` file exists** (verify:
`ls LICENSE*` from the repo root returns nothing, and `git ls-files | grep -i
license` is empty). GitHub therefore shows "no license detected," which blocks
corporate/legal adoption and makes the README's `[LICENSE](LICENSE)` link dead.

### Steps
1. Create a file named exactly `LICENSE` (no extension) at the repo root.
2. Fill it with the **full, standard Apache License 2.0 text**. Get the exact
   canonical text from https://www.apache.org/licenses/LICENSE-2.0.txt . Do not
   paraphrase or truncate — it must be the verbatim license.
3. In the copyright line of the appendix (`Copyright [yyyy] [name of copyright
   owner]`), you may leave the standard boilerplate as-is, or set it to
   `Copyright 2026 Chris Arter`. (Ask if unsure; leaving the boilerplate is
   acceptable and common.)
4. Optionally add a `NOTICE` file if desired — not required.

### Verify
- [ ] `test -f LICENSE && head -1 LICENSE` shows the Apache header line.
- [ ] The file is ~11 KB (the full license, not a stub).
- [ ] `git add LICENSE` — it is now tracked.

### Done when
`LICENSE` exists at the repo root with the complete Apache-2.0 text and is
staged for commit.

---

## Task 1.2 — `init --dry-run` must not write or bless anything  [Finding C4]

- **Severity:** Blocker · **Effort:** S · **Depends on:** none
- **Files:** `crates/ironlint-cli/src/commands/init/mod.rs`
  (anchors: `pub fn run`, `scaffold_config(dir)?`, `fn scaffold_config`)

### What's wrong
`ironlint init --dry-run` is documented as "Print intended changes without
writing," but it **actually writes `.ironlint.yml` and blesses it in the trust
store**. In `fn run`, `scaffold_config(dir)?` is called unconditionally;
`scaffold_config` both writes the config and calls `ironlint_core::trust::bless`
(search for `trust::bless` in that file). Only the *hook* phase respects
`dry_run`. A dry-run that mutates the security-critical trust store is
disqualifying for a trust-branded tool.

### Step 1 — Failing test first
Add a test to the `mod tests` block at the bottom of
`crates/ironlint-cli/src/commands/init/mod.rs` (there are already tests there —
search for `fn run_rejects_no_hook_and_hook_only_together` to find the block and
copy its `Options { .. }` construction style). The test must:
- Create a temp dir (use `tempfile::tempdir()`).
- Call `run(dir, &Options { dry_run: true, ..the-other-fields })` with a fresh
  `XDG_CONFIG_HOME`/`HOME` pointing into the temp dir so the real trust store is
  never touched (look at how existing tests isolate; if none do, set the env
  with `std::env::set_var` is unsafe across tests — instead prefer an
  integration test, see note below).
- Assert that **after** the call, `dir.join(".ironlint.yml")` does **not**
  exist.

> **Note on isolation:** unit tests share a process, and trust writes go to
> `XDG_CONFIG_HOME`. The cleaner home for this is an **integration test** under
> `crates/ironlint-cli/tests/` using `assert_cmd` with `.env("XDG_CONFIG_HOME",
> tmp)` (search the existing tests dir for `blessed_store` / `XDG_CONFIG_HOME`
> to copy the pattern). Prefer that. The test runs `ironlint init --dry-run` in
> a temp git repo and asserts `.ironlint.yml` was NOT created and the trust
> store file was NOT created.

Run it; it must **fail** (the file is currently created).

### Step 2 — The fix
In `fn run`, guard the scaffold call on `!opts.dry_run`. Find:
```rust
scaffold_config(dir)?;
```
and make it conditional, e.g.:
```rust
if opts.dry_run {
    // Render what WOULD happen, without touching disk or the trust store.
    println!("would scaffold and trust: {}", dir.join(".ironlint.yml").display());
} else {
    scaffold_config(dir)?;
}
```
Match the surrounding plan-rendering style if the file already prints a plan for
the hook phase (look at `render.rs` / `onboard.rs` for the dry-run plan format
and mirror it so the output is consistent). Do **not** call `scaffold_config`
(which writes + blesses) on the dry-run path.

Also fix the `--dry-run` help text if it over-promises — the CLI arg help lives
in `crates/ironlint-cli/src/cli.rs` (search for `dry_run` / `Print intended
changes`). It is now accurate, so likely no change needed, but confirm.

### Verify
- [ ] The new test passes.
- [ ] Manually (in a scratch dir, NOT the repo): `HOME=/tmp/x
      XDG_CONFIG_HOME=/tmp/x/.config ./target/debug/ironlint init --dry-run` in
      an empty temp dir prints a "would…" line and creates **no** files.
- [ ] Existing init tests still pass (`cargo test -p ironlint-cli init`).
- [ ] Standard Verification (README §Verify).

### Done when
`init --dry-run` writes nothing to disk and adds nothing to the trust store, and
a test guards it.

---

## Task 1.3 — Usage errors must exit 1, not 2  [Finding E4]

- **Severity:** Major · **Effort:** S · **Depends on:** none
- **Files:** `crates/ironlint-cli/src/main.rs` (clap parsing / error handling),
  `crates/ironlint-cli/src/cli.rs`, and the exit-code docs in
  `docs/reference/cli.md`.

### What's wrong
A command-line **usage error** (a typo'd flag like `--fiel`, a missing value,
or bare `ironlint`) currently exits with code **2**. But `2` is the documented
**Block** code. The Claude Code hook treats exit 2 as a block and shows its
(empty) stdout as the reason — so a typo becomes an *unexplained policy block*
the agent sees. Exit codes are this tool's primary contract; usage errors must
be distinguishable from a real Block.

### Step 1 — Failing test first
Add an integration test under `crates/ironlint-cli/tests/` (search for an
existing `cli_*` test that uses `Command::cargo_bin("ironlint")` to copy the
pattern). Assert that a bad invocation exits **1** (not 2):
```rust
Command::cargo_bin("ironlint").unwrap()
    .args(["check", "--fiel", "x"])   // typo'd flag
    .assert()
    .code(1);
```
Also add a case for bare `ironlint` (no subcommand). Run it — it must **fail**
today (currently exits 2).

### Step 2 — The fix
clap's default behavior exits `2` on parse errors. Change `main.rs` to parse
with the fallible API and map parse errors to exit **1** (config/usage tier).
Instead of `let cli = Cli::parse();`, do something like:
```rust
use clap::Parser;
let cli = match Cli::try_parse() {
    Ok(c) => c,
    Err(e) => {
        // Print clap's formatted help/error to the right stream, then exit 1.
        e.print().expect("write clap error");
        std::process::exit(1);
    }
};
```
Keep clap's own message formatting (`e.print()` handles stdout-for-help vs
stderr-for-error correctly). Only the **exit code** changes: 2 → 1.

> Do **not** change the `check` command's own 0/1/2/3 verdict codes — those are
> correct. This only affects the *argument-parsing* failure path.

### Step 3 — Docs
In `docs/reference/cli.md`, find the exit-code table (search for `| 2 |` or
`block`) and add a line clarifying that argument/usage errors exit `1`
(config/usage tier), so `2` is reserved for a real Block.

### Verify
- [ ] The new test passes; no path exits 2 except a real Block.
- [ ] `ironlint --help` and `ironlint check --help` still print help and exit 0.
- [ ] Standard Verification.

### Done when
Usage/parse errors exit 1, a test guards it, and the docs say so.

---

## Task 1.4 — Windows: fail loud when `sh` is unavailable  [Finding C2]

- **Severity:** Blocker · **Effort:** S (fail-loud slice; full Windows support is
  a later, larger effort) · **Depends on:** Task 1.6 (reuse the doctor-row pattern)
- **Files:** `crates/ironlint-cli/src/commands/check.rs` (startup, near
  `fn run` / the `ensure_trusted` call), `crates/ironlint-cli/src/commands/doctor.rs`.

### What's wrong
The engine spawns `Command::new("sh")` unconditionally (`crates/ironlint-core/
src/engine/gate.rs`, anchor `Command::new("sh")`). There is **no Windows code
path** anywhere in `crates/`. On stock Windows there is no `sh` on `PATH`, so
every check fails to spawn → exit 3 → adapters fail open. The user installs a
Windows binary (dist ships one) and enforces nothing, silently. We are **not**
building real Windows execution here — we are making it **fail loud** so nobody
is fooled.

### Step 1 — Failing test first
This is hard to unit-test cross-platform (you are likely on macOS/Linux where
`sh` exists). Write the test **guarded to Windows** so it documents intent and
runs in CI's Windows leg once that exists:
```rust
#[cfg(windows)]
#[test]
fn check_fails_loud_when_no_posix_shell() { /* assert exit 1 + message */ }
```
On non-Windows, also add a **unit test for the probe function** you write in
Step 2 (test the function directly with a fake "command name that doesn't
exist"), so the logic is covered on all platforms.

### Step 2 — The fix
1. Add a small helper (in `check.rs` or a shared util) `fn posix_shell_available()
   -> bool` that checks whether `sh` can be found/executed (e.g. attempt
   `Command::new("sh").arg("-c").arg("exit 0").status()` and treat an
   `ErrorKind::NotFound` as unavailable). Keep it under cognitive-complexity 15.
2. At the **start of `check::run`**, before doing real work, if the shell is not
   available, print a clear config-tier error to stderr and return exit **1**
   (NOT 3 — 3 makes adapters fail open; 1 is the loud config-error tier):
   ```
   error: no POSIX shell (`sh`) found on PATH. IronLint runs checks via `sh -c`
   and cannot enforce anything without it. On Windows, run IronLint inside Git
   Bash or WSL. See docs/getting-started.md.
   ```
3. In `doctor.rs`, add a row (reusing the row pattern from Task 1.6) that reports
   shell availability: `pass` if `sh` is found, else `fail` with the same
   remediation string.

### Step 3 — Docs
Add a short "Windows" note to `docs/getting-started.md`: IronLint requires a
POSIX shell; on Windows use Git Bash or WSL. Also add a `windows-latest` CI leg
— **but that belongs to Task 3.7** (CI changes are grouped there). Cross-
reference it; do not edit CI here.

### Verify
- [ ] Unit test for the probe passes on your platform.
- [ ] `ironlint doctor` shows a shell row = pass on your machine.
- [ ] Standard Verification.

### Done when
On a machine without `sh`, `ironlint check` exits 1 with a clear message and
`doctor` reports the missing shell — no silent fail-open.

---

## Task 1.5 — Reasonix adapter: honor `IRONLINT_FAIL_CLOSED_ON_INTERNAL`  [Finding E2]

- **Severity:** Major · **Effort:** S · **Depends on:** none
- **Files:** `adapters/reasonix/hooks/hook.sh` (anchor: the `case "${ec}" in`
  block, around the `*)` arm that does `exit 0`)
- **Type:** shell script (test via the adapter contract test — see note)

### What's wrong
The env var `IRONLINT_FAIL_CLOSED_ON_INTERNAL` is documented as the universal
"harden the gate" opt-in and is implemented in the claude-code, pi, and opencode
adapters — but **not** in reasonix (verify: `grep -c FAIL_CLOSED_ON_INTERNAL
adapters/reasonix/hooks/hook.sh` prints `0`; the other three print 3–4). An org
that sets it fleet-wide leaves reasonix silently fail-open on a crashed/timed-out
check, and reasonix is a *pre-write* gate that could actually block.

### Step 1 — See how the others do it
Read `adapters/claude-code/hooks/hook.sh` and search for
`FAIL_CLOSED_ON_INTERNAL`. Note exactly how it branches on the internal-error
exit code (3) and how it maps to a block vs allow. Mirror that logic.

### Step 2 — The fix
In `adapters/reasonix/hooks/hook.sh`, find the exit-code `case` (anchor
`case "${ec}" in`). It currently has:
```sh
case "${ec}" in
  0) exit 0 ;;
  2) ... exit 2 ;;               # block
  *)                              # <-- everything else, incl. exit 3
    echo "ironlint: internal error checking ${file} (exit ${ec})" >&2
    exit 0                        # <-- SILENT FAIL-OPEN
    ;;
esac
```
Change the catch-all so that **exit code 3** (internal error) honors the env
var, and **exit code 1** (config/untrusted error) is surfaced loudly too:
```sh
  3)
    if [ "${IRONLINT_FAIL_CLOSED_ON_INTERNAL:-0}" = "1" ]; then
      echo "ironlint: check errored (exit 3) — blocking (fail-closed)" >&2
      exit 2
    fi
    echo "ironlint: check errored (exit 3) — allowing (fail-open default)" >&2
    exit 0
    ;;
  1)
    echo "ironlint: config/trust error (exit 1) — see 'ironlint doctor'" >&2
    exit 0    # (Phase 3 Task 3.2 upgrades exit 1/4 handling; leave allow for now but LOUD)
    ;;
  *)
    echo "ironlint: unexpected ironlint exit ${ec} for ${file}" >&2
    exit 0
    ;;
```
Match the exact quoting/variable style already used in the file. Keep the
existing block (exit 2) arm unchanged.

### Step 2b — Note the reasonix python3 dependency
While here: the reasonix README lists `jq` as a requirement but omits `python3`
(the `edit_file` path shells out to it). Add `python3` to the requirements list
in `adapters/reasonix/README.md`. (Deeper payload-parse hardening is Task 5 /
E9 — not here.)

### Step 3 — Verify
- [ ] `grep -c FAIL_CLOSED_ON_INTERNAL adapters/reasonix/hooks/hook.sh` now ≥ 1.
- [ ] Manually simulate: create a fake `ironlint` on PATH that `exit 3`, pipe a
      minimal reasonix payload into `hook.sh`, and confirm: default → allow with
      a loud stderr line; `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1` → exit 2. (Do
      this in a scratch dir, not the repo.)
- [ ] `bash -n adapters/reasonix/hooks/hook.sh` (syntax check) passes.

> A proper automated contract test for this hook is **Task 3.9** — reference it,
> but the manual simulation above is enough to close this task.

### Done when
The reasonix hook honors `IRONLINT_FAIL_CLOSED_ON_INTERNAL` identically to the
other three adapters, and its exit-1/exit-3 handling is loud, not silent.

---

## Task 1.6 — `doctor`: add a trust row and a "no hooks wired" row  [Findings D3, partial C3]

- **Severity:** Major · **Effort:** S · **Depends on:** none
- **Files:** `crates/ironlint-cli/src/commands/doctor.rs`

### What's wrong
`ironlint doctor` reports "healthy" (all `[ok]`, exit 0) in a repo where **no
agent hook is wired** and where the **config is untrusted** — the two most
common first-run failure modes. The tool's whole job happens through hooks and
requires trust, so doctor greenlighting a no-op install is a real gap.

### Step 1 — Failing tests first
There are already doctor tests (search `crates/ironlint-cli/tests/` for
`cli_e2e_doctor` or `doctor`). Add two:
1. In a temp repo with a valid config but **no adapters installed and no trust
   entry**, run `ironlint doctor` and assert the output contains a warning about
   no hooks wired (e.g. text like `no coding-agent hooks detected`) — currently
   absent, so it fails.
2. In a temp repo with a config that is **not blessed**, assert doctor shows a
   trust row that is not `ok` and names `ironlint trust` as the remediation —
   currently absent, so it fails.

Run them; both must fail.

### Step 2 — The fix
`doctor.rs` builds a list of check rows (search for how existing rows are pushed
— there is a `remediation` field on the row type per the review). Add:
1. **Trust row:** call `ironlint_core::trust::ensure_trusted(config_path)` (it
   returns `Result<()>`). `Ok` → row `pass` ("config is trusted"). `Err` → row
   `warn`/`fail` with message "config/gates not trusted" and `remediation: run
   `ironlint trust``. Do not make doctor itself fail hard just because the config
   is untrusted — a `warn` that still lets doctor exit per its existing
   pass/warn/fail rules is right (read the file's exit logic and match it).
2. **Adapters/hooks row:** doctor already reports per-detected-harness rows, but
   emits **nothing** when zero harnesses are detected. Add an always-present row:
   if no coding-agent hooks are detected/installed, emit
   `warn: no coding-agent hooks detected — run 'ironlint init'`. If at least one
   is wired, `pass`.

Keep each new helper under cognitive-complexity 15 (extract a `fn trust_row(..)`
and `fn hooks_row(..)` rather than inlining into a giant function).

If doctor has a `--format json` path (it does — see `cli.rs`), make sure the two
new rows appear in the JSON output too, matching the existing row schema. Update
`docs/operating/diagnostics.md` if it enumerates the rows.

### Verify
- [ ] Both new tests pass.
- [ ] `ironlint doctor` in a fresh unblessed repo shows a trust warn + a
      no-hooks warn.
- [ ] `ironlint doctor --format json` includes both rows.
- [ ] Standard Verification.

### Done when
`doctor` can no longer report "healthy" when the config is untrusted or no hook
is wired.

---

## Task 1.7 — Fix the Claude Code "before it lands on disk" doc claim  [Finding E1, doc part]

- **Severity:** Major (honesty) · **Effort:** S · **Depends on:** none
- **Files:** `adapters/claude-code/README.md`, and any other doc claiming the
  claude-code adapter gates before the write (grep for `before it lands` and
  `before it` across `README.md`, `docs/`, `adapters/`).
- **Type:** doc-only (no test)

### What's wrong
The Claude Code adapter uses **PostToolUse** — it fires *after* the edit is
already on disk and can only feed a message back to the model; it does not
prevent or revert the write. But `adapters/claude-code/README.md` says it gates
"before it lands on disk." The tool's own `specs/2026-07-02-zcode-adapter-design.md`
(§D2) states the opposite plainly. This is an accuracy problem in the flagship
adapter's docs.

> The *code* migration to PreToolUse is the larger **Task 3.1**. This task only
> corrects the docs so they are honest *today*.

### Steps
1. In `adapters/claude-code/README.md`, replace claims of pre-write gating with
   an accurate description: the adapter runs on **PostToolUse**, so the edit has
   already been written; a block is surfaced to the agent as feedback (exit 2 +
   the check's message), and the agent is expected to correct it on the next
   turn. Note that pre-write blocking is planned (Task 3.1 / the reasonix and
   zcode adapters already gate pre-write).
2. Grep the rest of the repo for the same overclaim and fix each occurrence:
   ```bash
   grep -rn "before it lands" README.md docs/ adapters/
   ```
   The root `README.md` "Runs on the write, not after" framing is about the
   *product's write lifecycle* generally — leave the general pitch, but make sure
   nothing specifically says the **Claude Code adapter** blocks before disk.

### Verify
- [ ] `grep -rn "before it lands on disk" adapters/claude-code/` returns nothing.
- [ ] The claude-code README now names PostToolUse and describes after-the-fact
      feedback honestly.

### Done when
No doc claims the Claude Code adapter blocks a write before it lands; the
PostToolUse reality is described accurately, with pre-write noted as planned.
