# Script-Engine Pre-Write Content Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `engine: script` rules evaluate the caller-supplied proposed content (via `--content`) instead of the on-disk file, so pre-write gates (Reasonix, OpenCode `before`) actually check the edit that's about to land.

**Architecture:** The runner already carries the proposed content in `RuleContext.content` and hands it to every engine; only the `script` engine ignores it (it shells out against the on-disk `{file}`). The fix is a single agnostic primitive — **core offers the proposed content on the script subprocess's stdin** — implemented in the subprocess spawn (`capability.rs`). The author opts in by writing their tool's stdin form (`biome check --stdin-file-path={file}`); `{file}` stays a path/extension *hint*. Lifecycle stays in the adapter, tool variability stays in `.hector.yml`, core learns nothing tool-specific. See `specs/2026-05-29-script-engine-prewrite-content.md` (esp. §9 + §9.1 investigator's amendment) for the full rationale.

**Tech Stack:** Rust (cargo workspace: `hector-core` lib + `hector-cli` bin), `std::process` + raw `clone(2)` (nix) for the sandboxed spawn, `wait_timeout`, `assert_cmd` for CLI tests, bash + jq + python3 for the Reasonix adapter hook.

---

## Constraints (read before starting — from CLAUDE.md + spec §10)

- **Coverage gate:** Rust files under `crates/*/src/` must hit ≥80% **region** coverage, enforced per-file in CI (`scripts/ci-coverage.sh`). New code must bring its file to the gate. *Local caveat:* the maintainer's box (Homebrew rustc, no `llvm-tools-preview`) can't run `ci-coverage.sh` locally, and the Linux `cfg` paths in `capability.rs` can't be cross-compiled locally — **Task 3 (Linux clone path) is CI-verified only.** Don't claim local proof for it.
- **Cognitive complexity ≤ 15 per function** (clippy). The spawn functions are already dense; the `spawn_stdin_writer` helper (Task 1) exists precisely to keep them under the cap. Refactor over `#[allow]`.
- **TDD:** every behavior change starts with a failing test. The CLI seed tests already exist (`crates/hector-cli/tests/cli_check_content_script_prewrite.rs`, currently RED) — Task 2 turns them green.
- **Verdict JSON shape is locked** (`verdict.rs`). This change is behavior-only; no verdict/telemetry schema change. (`RuleExplain` in Task 7 is *not* part of the verdict JSON — see runner.rs:101.)
- **Don't clobber unrelated WIP:** `commands/check.rs`, `adapters/claude-code/hooks/hook.sh`+README, and two test files have in-flight edits for a *different* feature (claude-code `--diff` gating). This plan does not touch those. Verify `git status` before committing each task — stage only the files the task names.
- **Binary is `hector`**, not `hector-cli`. `Cargo.lock` is gitignored — never commit it.
- **Clean up build artifacts** this work produces (e.g. a stray `cargo mutants` run) per the cleanup-build-artifacts skill; the persistent `target/` stays.

## Setup (execution-time, before Task 1)

If not already in an isolated workspace, the executor should create one via the `superpowers:using-git-worktrees` skill, branching from `main`. Suggested branch: `feat/script-engine-prewrite-stdin`.

## Dependency graph

- **Task 1 → Task 2** (Task 2 calls `run_with_capabilities_stdin` from Task 1).
- **Task 1 → Task 3** (Task 3 reuses `spawn_stdin_writer` from Task 1).
- **Tasks 4, 5, 6, 7** are independent of each other and of 1–3's internals (4 is a pure adapter/bash change; 5/6 are scaffold/docs; 7 is the explain surface). Run them after 1–3 land so their end-to-end assertions hold.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `crates/hector-core/src/engine/capability.rs` | Modify | Add `run_with_capabilities_stdin` + `spawn_stdin_writer` helper; thread `stdin: Option<&[u8]>` through the macOS/fast path (`spawn_with_timeout`) and the Linux clone path (`run_linux`→`spawn_clone_with_timeout`→`spawn_clone`/`child_exec`/`wait_for_child`). |
| `crates/hector-core/src/engine/script.rs` | Modify | Thread `ctx.content` from `ScriptEngine::run` → `run_script_rule` → the capability call. |
| `crates/hector-core/tests/capability.rs` | Modify | Unit tests: content piped to a stdin-reading command (fast path); a command that ignores stdin doesn't hang/error; Linux clone-path stdin test. |
| `crates/hector-cli/tests/cli_check_content_script_prewrite.rs` | Exists (RED) | Integration seed — goes green in Task 2. |
| `adapters/reasonix/hooks/hook.sh` | Modify | Byte-exact proposed content: `jq -j` (write_file) and the append-sentinel idiom (edit_file) so trailing newlines survive. |
| `crates/hector-cli/tests/adapter_reasonix.rs` | Modify | Golden test: the hook pipes byte-exact content (trailing newline preserved) for write_file and edit_file. |
| `crates/hector-cli/src/commands/init/mod.rs` | Modify | Scaffold linter rules in **stdin form** so a fresh `hector init` gates correctly pre-write. |
| `crates/hector-cli/src/cli.rs` | Modify | `--content` help text: flip the "Limitation: script rules see on-disk content" note to the new contract. |
| `adapters/reasonix/README.md`, `specs/2026-05-25-reasonix-adapter.md`, `specs/2026-05-29-script-engine-prewrite-content.md` | Modify | Docs honesty: limitation → resolved; record the per-tool (not per-harness) boundary. |
| `crates/hector-core/src/runner.rs`, `crates/hector-cli/src/commands/check.rs` | Modify (Task 7, optional) | `--explain` advisory note for `{file}`-referencing script rules under authoritative `--content`. |

**Out of scope (explicitly deferred, YAGNI):** hoisting the edit→content synthesis (search/replace → full content) out of adapter hooks into a core `hector apply-edit`/`--edit-search` helper. Noted in spec §9.1 as a future candidate; not required for the "so what" and not in this plan.

---

## Phase 1 — Core: the script engine sees proposed content

### Task 1: stdin writer helper + fast-path piping

**Files:**
- Modify: `crates/hector-core/src/engine/capability.rs` (add `spawn_stdin_writer`; add `run_with_capabilities_stdin`; rewrite `run_with_capabilities`, `run_with_capabilities_env`, `run_best_effort_macos`, `spawn_with_timeout` to thread `stdin`)
- Test: `crates/hector-core/tests/capability.rs`

This task covers the macOS / no-isolation path only (the default `network: true` fast path, runnable locally). The Linux clone path is Task 3.

- [ ] **Step 1: Write the failing tests**

Append to `crates/hector-core/tests/capability.rs`:

```rust
#[test]
fn pipes_stdin_content_to_command() {
    // `cat` echoes whatever it reads from stdin. With the new stdin parameter,
    // the proposed content must reach the child.
    let caps = Capabilities {
        network: true, // keep on the fast path (no clone) so this runs anywhere
        writes: WritesPolicy::None,
    };
    let out = run_with_capabilities_stdin("cat", std::path::Path::new("."), &caps, &[], Some(b"hello stdin"))
        .expect("run");
    assert_eq!(out.stdout, "hello stdin");
    assert_eq!(out.exit_code, 0);
}

#[test]
fn ignored_stdin_does_not_hang_or_error() {
    // A command that never reads stdin must still complete cleanly. The writer
    // swallows BrokenPipe; a payload larger than the OS pipe buffer (64 KiB)
    // proves the writer thread can't deadlock the wait.
    let caps = Capabilities {
        network: true,
        writes: WritesPolicy::None,
    };
    let big = "x".repeat(256 * 1024);
    let out = run_with_capabilities_stdin("echo done", std::path::Path::new("."), &caps, &[], Some(big.as_bytes()))
        .expect("run");
    assert_eq!(out.stdout.trim(), "done");
    assert_eq!(out.exit_code, 0);
}

#[test]
fn none_stdin_preserves_legacy_behavior() {
    // No stdin supplied → child inherits parent fd 0 as before; a plain command
    // still runs.
    let caps = Capabilities {
        network: true,
        writes: WritesPolicy::None,
    };
    let out = run_with_capabilities_stdin("echo legacy", std::path::Path::new("."), &caps, &[], None)
        .expect("run");
    assert_eq!(out.stdout.trim(), "legacy");
    assert_eq!(out.exit_code, 0);
}
```

Add the import at the top of `crates/hector-core/tests/capability.rs` (next to the existing `run_with_capabilities` import):

```rust
use hector_core::engine::capability::run_with_capabilities_stdin;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p hector-core --test capability pipes_stdin_content_to_command ignored_stdin_does_not_hang_or_error none_stdin_preserves_legacy_behavior`
Expected: FAIL — compile error `cannot find function run_with_capabilities_stdin in module ... capability` (the function doesn't exist yet).

- [ ] **Step 3: Add the `spawn_stdin_writer` helper**

In `crates/hector-core/src/engine/capability.rs`, add this helper next to `spawn_reader` (after `join_reader`, ~line 443):

```rust
/// Feed `bytes` to a child's stdin on a dedicated thread, then close the pipe
/// (by dropping `writer`) to deliver EOF. Runs concurrently with the
/// stdout/stderr readers, so a child that streams output while reading stdin
/// cannot deadlock us.
///
/// A `BrokenPipe` write error is expected and swallowed: a rule whose command
/// ignores stdin (e.g. `grep PATTERN {file}`) closes the read-end early. This
/// relies on Rust's default `SIGPIPE` disposition (`SIG_IGN`) — the write
/// returns `BrokenPipe` rather than terminating the process with a signal.
fn spawn_stdin_writer<W: std::io::Write + Send + 'static>(
    mut writer: W,
    bytes: Vec<u8>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _ = writer.write_all(&bytes);
        // `writer` drops here → the pipe write-end closes → child sees EOF.
    })
}
```

- [ ] **Step 4: Add the public `run_with_capabilities_stdin` and re-point the existing wrappers**

Replace the existing `run_with_capabilities` (lines ~35-37) and `run_with_capabilities_env` (lines ~45-59) with:

```rust
/// Run a shell command under the requested capability constraints.
/// Returns Ok(outcome) if the process completed (even non-zero exit).
pub fn run_with_capabilities(cmd: &str, cwd: &Path, caps: &Capabilities) -> Result<ExecOutcome> {
    run_with_capabilities_env(cmd, cwd, caps, &[])
}

/// Same as [`run_with_capabilities`], with an extra env-injection slice.
///
/// Each `(name, value)` pair is applied to the child process environment
/// before spawning. Used by the script engine to pass attacker-controlled
/// values (like file paths) to the shell via env vars rather than splicing
/// them into the command text.
pub fn run_with_capabilities_env(
    cmd: &str,
    cwd: &Path,
    caps: &Capabilities,
    env: &[(&str, &str)],
) -> Result<ExecOutcome> {
    run_with_capabilities_stdin(cmd, cwd, caps, env, None)
}

/// Same as [`run_with_capabilities_env`], plus optional bytes piped to the
/// child's stdin. `Some(bytes)` is the proposed-content path the script engine
/// uses for pre-write gating: the command's stdin carries the bytes to check,
/// while a path/extension *hint* stays available via env (`HECTOR_FILE`/`{file}`).
/// `None` preserves the historical behavior (child inherits the parent's fd 0).
pub fn run_with_capabilities_stdin(
    cmd: &str,
    cwd: &Path,
    caps: &Capabilities,
    env: &[(&str, &str)],
    stdin: Option<&[u8]>,
) -> Result<ExecOutcome> {
    #[cfg(target_os = "linux")]
    {
        run_linux(cmd, cwd, caps, env, stdin)
    }
    #[cfg(not(target_os = "linux"))]
    {
        run_best_effort_macos(cmd, cwd, caps, env, stdin)
    }
}
```

- [ ] **Step 5: Thread `stdin` through `run_best_effort_macos` and `spawn_with_timeout`**

Replace `run_best_effort_macos` (lines ~386-399) with:

```rust
#[cfg(not(target_os = "linux"))]
fn run_best_effort_macos(
    cmd: &str,
    cwd: &Path,
    _caps: &Capabilities,
    env: &[(&str, &str)],
    stdin: Option<&[u8]>,
) -> Result<ExecOutcome> {
    // No eprintln here. The platform-best-effort story is surfaced by
    // `hector doctor` (see `platform_capability_status` and
    // `commands::doctor::check_capabilities`), not by every `check`
    // invocation: a per-process AtomicBool dedup still leaks to users because
    // the Claude Code adapter hook spawns ~3 hector processes per edit.
    spawn_with_timeout(cmd, cwd, env, stdin)
}
```

Replace `spawn_with_timeout` (lines ~452-502) with:

```rust
fn spawn_with_timeout(
    cmd: &str,
    cwd: &Path,
    env: &[(&str, &str)],
    stdin: Option<&[u8]>,
) -> Result<ExecOutcome> {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Only pipe stdin when there's content to feed; otherwise leave the default
    // (inherit parent fd 0) so content-less calls behave exactly as before.
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    }
    for (k, v) in env {
        command.env(k, v);
    }
    let mut child = command.spawn().context("spawning script subprocess")?;

    // Feed proposed content on stdin from a dedicated thread (see
    // spawn_stdin_writer). `take()` moves the handle into the thread, which
    // drops it after writing to deliver EOF.
    let stdin_writer = match (stdin, child.stdin.take()) {
        (Some(bytes), Some(si)) => Some(spawn_stdin_writer(si, bytes.to_vec())),
        _ => None,
    };

    // Drain both streams on dedicated threads BEFORE waiting, so a child that
    // writes past the pipe buffer never blocks on write(2). Each reader is
    // capped at MAX_OUTPUT and ends when the child's write-end closes.
    let stdout_reader = child.stdout.take().map(spawn_reader);
    let stderr_reader = child.stderr.take().map(spawn_reader);

    let status = child
        .wait_timeout(TIMEOUT)
        .context("waiting for subprocess")?;

    let Some(status) = status else {
        // Timeout fired. Kill and reap the direct child, then DETACH the reader
        // and writer threads rather than joining them: if the script
        // backgrounded a process that inherited a pipe fd, a join would hang us
        // past the very deadline this timeout enforces.
        let _ = child.kill();
        let _ = child.wait();
        drop(stdout_reader);
        drop(stderr_reader);
        drop(stdin_writer);
        return Ok(ExecOutcome {
            stdout: String::new(),
            stderr: format!("hector: script killed after {TIMEOUT:?} timeout"),
            exit_code: TIMEOUT_EXIT_CODE,
        });
    };

    let stdout = stdout_reader.map(join_reader).unwrap_or_default();
    let stderr = stderr_reader.map(join_reader).unwrap_or_default();
    // Detach the writer; its write result never affects the verdict, and the
    // child has exited so a blocked write has already unblocked via EPIPE.
    drop(stdin_writer);

    Ok(ExecOutcome {
        stdout,
        stderr,
        exit_code: status.code().unwrap_or(-1),
    })
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p hector-core --test capability`
Expected: PASS — all tests in the file, including the three new ones, green. (On macOS the clone path is `cfg`-excluded; on Linux it will also build — Task 3 adds its piping.)

> Linux note: after this step the Linux build still compiles because `run_linux`/`spawn_clone_with_timeout` still have their **old** signatures and `run_with_capabilities_stdin` calls `run_linux(cmd, cwd, caps, env, stdin)` — which won't compile until Task 3. **If you are on Linux, do Task 1 + Task 3 together before running `cargo test`** (the `run_linux` signature must change in lockstep). On macOS, Task 1 compiles and tests independently.

- [ ] **Step 7: Lint and commit**

Run: `cargo clippy -p hector-core --all-targets -- -D warnings` (confirm no cognitive-complexity warning on `spawn_with_timeout`; if it trips, extract the writer-setup match into a small helper rather than `#[allow]`).
Run: `cargo fmt`

```bash
git add crates/hector-core/src/engine/capability.rs crates/hector-core/tests/capability.rs
git commit -m "feat(script): offer proposed content on subprocess stdin (fast path)

Add run_with_capabilities_stdin + spawn_stdin_writer. The macOS/no-isolation
spawn now pipes caller content to the child's stdin (EOF-on-close,
EPIPE-tolerant, detached on timeout). None preserves legacy fd-0 inheritance.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: thread `ctx.content` through the script engine

**Files:**
- Modify: `crates/hector-core/src/engine/script.rs:11-21` (`ScriptEngine::run`), `:31-51` (`run_script_rule`)
- Test: `crates/hector-cli/tests/cli_check_content_script_prewrite.rs` (already exists, currently RED)

- [ ] **Step 1: Confirm the seed tests are red for the right reason**

Run: `cargo test -p hector-cli --test cli_check_content_script_prewrite`
Expected: FAIL — `script_proposed_content_is_checked_not_disk_blocks` expects exit 2/`block` but gets exit 0/`pass`; `script_clean_proposed_content_passes_despite_dirty_disk` expects exit 0/`pass` but gets exit 2/`block`. (This is the §4 repro encoded; it fails because the engine still reads disk.)

- [ ] **Step 2: Pass `ctx.content` into `run_script_rule`**

In `crates/hector-core/src/engine/script.rs`, replace `ScriptEngine::run` (lines 11-21):

```rust
impl RuleEngine for ScriptEngine {
    fn run(&self, ctx: &RuleContext) -> Result<Vec<Violation>> {
        run_script_rule(
            ctx.rule_id,
            ctx.rule,
            ctx.file,
            ctx.diff.unwrap_or(""),
            ctx.content,
            ctx.cwd,
        )
    }
}
```

- [ ] **Step 3: Accept content in `run_script_rule` and pipe it to the subprocess**

Replace the `run_script_rule` signature + capability call (lines 31-51). Change the import on line 2 and the call:

Line 2 — swap the import:
```rust
use crate::engine::capability::run_with_capabilities_stdin;
```

Signature + body head (lines 31-51):
```rust
pub fn run_script_rule(
    rule_id: &str,
    rule: &Rule,
    file: &Path,
    _diff: &str,
    content: Option<&str>,
    cwd: &Path,
) -> Result<Vec<Violation>> {
    let script = rule
        .script
        .as_ref()
        .ok_or_else(|| anyhow!("rule {rule_id} is engine: script but has no `script:` field"))?;
    // `{file}` expands to the shell parameter `"$HECTOR_FILE"`. The actual path
    // is passed via the child environment, never spliced into the command
    // text, so shell metacharacters in the filename cannot escape into the
    // surrounding command. The double-quotes prevent word-splitting on
    // whitespace in the path.
    let substituted = script.replace("{file}", "\"$HECTOR_FILE\"");
    let caps = rule.capabilities.clone().unwrap_or_default();
    let file_str = file.display().to_string();
    // Offer the proposed content on the command's stdin. A stdin-form command
    // (`biome check --stdin-file-path={file}`, `ruff check --stdin-filename {file} -`)
    // checks the proposed edit; a path-only `{file}` command ignores the bytes
    // and reads disk (the writer tolerates the resulting EPIPE).
    let outcome = run_with_capabilities_stdin(
        &substituted,
        cwd,
        &caps,
        &[("HECTOR_FILE", &file_str)],
        content.map(str::as_bytes),
    )?;
    if outcome.exit_code == 0 {
        return Ok(Vec::new());
    }
    // ... rest of function unchanged ...
```

(Leave everything after the `if outcome.exit_code == 0` line untouched.)

- [ ] **Step 4: Run the seed tests to verify they pass**

Run: `cargo test -p hector-cli --test cli_check_content_script_prewrite`
Expected: PASS — both tests green. The block case now blocks on piped `FORBIDDEN`; the pass case now passes on clean piped content despite the dirty disk file.

- [ ] **Step 5: Run the broader engine + script tests for regressions**

Run: `cargo test -p hector-core script` and `cargo test -p hector-cli`
Expected: PASS — no regressions. (Existing post-write `--file` and `--diff` script tests still pass because `content` is `Some(disk_bytes)` in those modes, so a stdin-form rule checks the same bytes it always did.)

- [ ] **Step 6: Lint and commit**

Run: `cargo clippy --all-targets -- -D warnings` then `cargo fmt`

```bash
git add crates/hector-core/src/engine/script.rs
git commit -m "fix(script): evaluate proposed --content, not the on-disk file

Thread ctx.content to the subprocess stdin so pre-write gates check the edit
about to land. Closes the §4 repro; the existing seed tests go green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Linux clone-path stdin piping (CI-verified only)

**Files:**
- Modify: `crates/hector-core/src/engine/capability.rs` — `run_linux`, `spawn_clone_with_timeout`, `spawn_clone`, `child_exec`, `wait_for_child`
- Test: `crates/hector-core/tests/capability.rs` (Linux-gated)

> **You cannot verify this locally on macOS/Homebrew-rustc.** It compiles under `#[cfg(target_os = "linux")]` only and is exercised in CI. Reason carefully; the child runs after raw `clone(2)` and must stay async-signal-safe (no alloc, no locks) until `execve`. The only new child-side syscall is one `dup2`, which is async-signal-safe.

- [ ] **Step 1: Write the failing (Linux-gated) test**

Append to `crates/hector-core/tests/capability.rs`:

```rust
#[cfg(target_os = "linux")]
#[test]
fn pipes_stdin_on_clone_path_with_network_isolation() {
    // network: false forces the clone(2) path (CLONE_NEWNET). The proposed
    // content must reach the child there too. On an unprivileged runner the
    // clone EPERM-falls-back to the fast path, which also pipes stdin — so
    // either way `cat` must echo the piped bytes.
    let caps = Capabilities {
        network: false,
        writes: WritesPolicy::None,
    };
    let out = run_with_capabilities_stdin("cat", std::path::Path::new("."), &caps, &[], Some(b"clone stdin"))
        .expect("run");
    assert!(
        out.stdout.contains("clone stdin"),
        "clone-path child must receive piped stdin; got stdout={:?} exit={}",
        out.stdout,
        out.exit_code
    );
}
```

- [ ] **Step 2: (CI) Verify it fails to compile / fails**

This won't build locally on macOS. On a Linux checkout: `cargo test -p hector-core --test capability pipes_stdin_on_clone_path_with_network_isolation`
Expected: FAIL — `run_linux`/`spawn_clone*` signatures don't yet accept `stdin` (compile error), or after Step 3 partial edits, a behavioral failure until the child dup2 lands.

- [ ] **Step 3: Thread `stdin` through `run_linux` and `spawn_clone_with_timeout`**

Replace `run_linux` (lines ~71-97) signature + the two tail calls:

```rust
#[cfg(target_os = "linux")]
fn run_linux(
    cmd: &str,
    cwd: &Path,
    caps: &Capabilities,
    env: &[(&str, &str)],
    stdin: Option<&[u8]>,
) -> Result<ExecOutcome> {
    use nix::sched::CloneFlags;

    let mut flags = CloneFlags::empty();
    if !caps.network {
        flags.insert(CloneFlags::CLONE_NEWNET);
    }

    if flags.is_empty() {
        return spawn_with_timeout(cmd, cwd, env, stdin);
    }

    spawn_clone_with_timeout(cmd, cwd, env, flags, stdin)
}
```

Replace `spawn_clone_with_timeout` (lines ~107-134):

```rust
#[cfg(target_os = "linux")]
fn spawn_clone_with_timeout(
    cmd: &str,
    cwd: &Path,
    env: &[(&str, &str)],
    flags: nix::sched::CloneFlags,
    stdin: Option<&[u8]>,
) -> Result<ExecOutcome> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);

    let (child_pid, stdin_w, stdout_r, stderr_r) =
        match spawn_clone(cmd, cwd, env, flags, stdin.is_some()) {
            Ok(quad) => quad,
            Err(err) => {
                if !WARNED.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "hector: capability sandbox unavailable for unprivileged user ({err}); \
                         running command without isolation. See docs/security/capabilities.md."
                    );
                }
                // Fallback path pipes stdin too.
                return spawn_with_timeout(cmd, cwd, env, stdin);
            }
        };
    wait_for_child(child_pid, stdin_w, stdin.map(<[u8]>::to_vec), stdout_r, stderr_r)
}
```

- [ ] **Step 4: Create the stdin pipe in `spawn_clone` and return its write-end**

In `spawn_clone` (lines ~148-269): change the return type and add the conditional stdin pipe.

Signature (line ~149-154) becomes:
```rust
#[cfg(target_os = "linux")]
fn spawn_clone(
    cmd: &str,
    cwd: &Path,
    env: &[(&str, &str)],
    flags: nix::sched::CloneFlags,
    has_stdin: bool,
) -> Result<(
    nix::unistd::Pid,
    Option<std::os::fd::OwnedFd>,
    std::os::fd::OwnedFd,
    std::os::fd::OwnedFd,
)> {
```

After the existing stdout/stderr `pipe2` calls (lines ~162-163), add:
```rust
    // Conditionally create a stdin pipe. When absent, the child inherits the
    // parent's fd 0 exactly as before — no behavior change for content-less calls.
    let stdin_pipe = if has_stdin {
        Some(pipe2(OFlag::O_CLOEXEC).context("pipe2 for child stdin")?)
    } else {
        None
    };
    let stdin_r_raw: Option<std::os::fd::RawFd> =
        stdin_pipe.as_ref().map(|(r, _)| r.as_raw_fd());
```

Change the child closure (lines ~225-236) to pass `stdin_r_raw` into `child_exec` as the first arg:
```rust
    let child_fn: nix::sched::CloneCb<'_> = Box::new(move || -> isize {
        child_exec(
            stdin_r_raw,
            stdout_w_raw,
            stderr_w_raw,
            &cwd_c,
            &sh_path,
            &argv,
            &envp,
            &argv_ptrs,
            &envp_ptrs,
        )
    });
```

After `Box::leak(stack);` and the existing `drop(stdout_w); drop(stderr_w);` (lines ~260-266), drop the parent's stdin read-end and keep the write-end, then return the quad:
```rust
    // Drop the parent's copy of the stdin read-end (the child holds its own
    // COW copy and dup2's it to fd 0); keep the write-end for the writer thread.
    let stdin_w = stdin_pipe.map(|(stdin_r, stdin_w)| {
        drop(stdin_r);
        stdin_w
    });

    Ok((pid, stdin_w, stdout_r, stderr_r))
```

- [ ] **Step 5: Add the conditional `dup2` of stdin in `child_exec`**

Replace `child_exec` (lines ~283-314) signature + the dup2 prologue:

```rust
#[cfg(target_os = "linux")]
fn child_exec(
    stdin_r_raw: Option<std::os::fd::RawFd>,
    stdout_w_raw: std::os::fd::RawFd,
    stderr_w_raw: std::os::fd::RawFd,
    cwd: &std::ffi::CStr,
    sh_path: &std::ffi::CStr,
    _argv: &[std::ffi::CString],
    _envp: &[std::ffi::CString],
    argv_ptrs: &[*const libc::c_char],
    envp_ptrs: &[*const libc::c_char],
) -> isize {
    use nix::unistd::{chdir, dup2};

    // dup2 is async-signal-safe (a bare syscall, no alloc/lock) — safe in the
    // post-clone child. Sources are pipe fds (>= 3 in the parent), so duping
    // onto 0/1/2 never clobbers another source.
    if let Some(fd) = stdin_r_raw {
        if dup2(fd, 0).is_err() {
            return 126;
        }
    }
    if dup2(stdout_w_raw, 1).is_err() || dup2(stderr_w_raw, 2).is_err() {
        return 126;
    }
    if chdir(cwd).is_err() {
        return 126;
    }
    // SAFETY: unchanged from before — parent-built CStrings/pointer arrays,
    // async-signal-safe execve, no allocation.
    #[allow(unsafe_code)]
    unsafe {
        libc::execve(sh_path.as_ptr(), argv_ptrs.as_ptr(), envp_ptrs.as_ptr());
    }
    127
}
```

- [ ] **Step 6: Spawn the writer in `wait_for_child`**

Replace `wait_for_child` signature (lines ~323-328) and add the writer alongside the readers:

```rust
#[cfg(target_os = "linux")]
fn wait_for_child(
    pid: nix::unistd::Pid,
    stdin_w: Option<std::os::fd::OwnedFd>,
    stdin_bytes: Option<Vec<u8>>,
    stdout_r: std::os::fd::OwnedFd,
    stderr_r: std::os::fd::OwnedFd,
) -> Result<ExecOutcome> {
    use nix::sys::signal::{kill, Signal};
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};

    let stdout_reader = spawn_reader(std::fs::File::from(stdout_r));
    let stderr_reader = spawn_reader(std::fs::File::from(stderr_r));
    // Feed stdin concurrently with the readers; drop the write-end after
    // writing to deliver EOF. None when the call supplied no content.
    let stdin_writer = match (stdin_w, stdin_bytes) {
        (Some(w), Some(bytes)) => Some(spawn_stdin_writer(std::fs::File::from(w), bytes)),
        _ => None,
    };

    let deadline = std::time::Instant::now() + TIMEOUT;
    let poll_interval = std::time::Duration::from_millis(10);

    let exit_status = loop {
        match waitpid(pid, Some(WaitPidFlag::WNOHANG)).context("waitpid on cloned child")? {
            WaitStatus::StillAlive => {
                if std::time::Instant::now() >= deadline {
                    let _ = kill(pid, Signal::SIGKILL);
                    let _ = waitpid(pid, None);
                    drop(stdout_reader);
                    drop(stderr_reader);
                    drop(stdin_writer);
                    return Ok(ExecOutcome {
                        stdout: String::new(),
                        stderr: format!("hector: script killed after {TIMEOUT:?} timeout"),
                        exit_code: TIMEOUT_EXIT_CODE,
                    });
                }
                std::thread::sleep(poll_interval);
            }
            other => break other,
        }
    };

    let stdout = join_reader(stdout_reader);
    let stderr = join_reader(stderr_reader);
    drop(stdin_writer);
    Ok(ExecOutcome {
        stdout,
        stderr,
        exit_code: exit_status_to_code(exit_status),
    })
}
```

- [ ] **Step 7: (CI) Verify the test passes and clippy is clean**

On a Linux checkout / in CI: `cargo test -p hector-core --test capability` and `cargo clippy -p hector-core --all-targets -- -D warnings`.
Expected: PASS, including `pipes_stdin_on_clone_path_with_network_isolation`. Watch the cognitive-complexity cap on `spawn_clone`/`wait_for_child`; if either trips, extract a helper rather than `#[allow]`.

- [ ] **Step 8: Commit**

```bash
git add crates/hector-core/src/engine/capability.rs crates/hector-core/tests/capability.rs
git commit -m "feat(script): pipe proposed content on the Linux clone(2) path

Conditional O_CLOEXEC stdin pipe + one async-signal-safe dup2(stdin,0) in the
post-clone child; parent writer thread feeds it. Content-less calls keep
inheriting fd 0. CI-verified (cannot cross-compile the cfg path locally).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Phase 2 — Don't ship a half-fix: adapter content fidelity

### Task 4: byte-exact proposed content in the Reasonix hook

**Files:**
- Modify: `adapters/reasonix/hooks/hook.sh:106` (write_file), `:119-144` (edit_file)
- Test: `crates/hector-cli/tests/adapter_reasonix.rs`

**Why:** With Phase 1 live, the hook's synthesized content finally reaches the tool — so its byte errors become live diagnostics. `jq -r` (write_file) *appends* a trailing newline; `$(python3 …)` (edit_file) *strips* trailing newlines. Both must preserve bytes exactly, or biome/eslint emit spurious "missing/extra final newline" findings.

- [ ] **Step 1: Write the failing golden test**

Read `crates/hector-cli/tests/adapter_reasonix.rs` first to match its existing harness (how it invokes `hook.sh`, sets `PATH`/cwd, and builds the JSON payload). Then add a test that puts a **stub `hector`** on `PATH` which copies its stdin to a capture file, runs the hook with a `write_file` payload whose `content` ends in a trailing newline, and asserts the captured bytes equal the payload content **exactly** (including the trailing `\n`). Add a sibling test for `edit_file` (search/replace producing content that ends in `\n`).

Concrete shape (adapt names/paths to the file's existing helpers):

```rust
#[test]
fn write_file_pipes_byte_exact_content_including_trailing_newline() {
    let tmp = tempfile::tempdir().unwrap();
    let capture = tmp.path().join("captured_stdin");
    // Stub `hector`: copy stdin to the capture file, exit 0 (pass).
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        bin.join("hector"),
        format!("#!/usr/bin/env bash\ncat > \"{}\"\nexit 0\n", capture.display()),
    )
    .unwrap();
    // chmod +x
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(bin.join("hector"), std::fs::Permissions::from_mode(0o755)).unwrap();

    // Minimal trusted .hector.yml so the hook doesn't early-exit on a missing config.
    std::fs::write(
        tmp.path().join(".hector.yml"),
        "schema_version: 2\nrules: {}\n",
    )
    .unwrap();

    let expected = "export const x = 1;\n"; // note trailing newline
    let payload = serde_json::json!({
        "event": "PreToolUse",
        "cwd": tmp.path(),
        "toolName": "write_file",
        "toolArgs": { "path": "src/x.ts", "content": expected },
    })
    .to_string();

    // Run the hook with the stub `hector` first on PATH, writing the payload
    // to its stdin (the hook reads the event JSON from stdin via `cat`).
    let hook = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/reasonix/hooks/hook.sh");
    let path_env = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let mut child = std::process::Command::new("bash")
        .arg(&hook)
        .env("PATH", path_env)
        .current_dir(tmp.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(payload.as_bytes()).unwrap();
    } // stdin drops here → EOF → hook's `cat` returns
    child.wait_with_output().unwrap();

    let captured = std::fs::read_to_string(&capture).unwrap();
    assert_eq!(captured, expected, "hook must pipe byte-exact content");
}
```

> If `adapter_reasonix.rs` already has a helper that runs the hook with a JSON payload, prefer it over re-rolling the spawn above — match the file's idiom. Add the matching `edit_file` test: seed `src/x.ts` on disk with content containing a unique `search` substring whose `replace` yields a result ending in `\n`, and assert the captured bytes equal that result exactly.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p hector-cli --test adapter_reasonix write_file_pipes_byte_exact`
Expected: FAIL — `jq -r` appended a `\n`, so `captured` is `"...1;\n\n"` ≠ `"...1;\n"`. (The edit_file test fails the other way: `$()` stripped the trailing `\n`.)

- [ ] **Step 3: Fix write_file — use `jq -j` (raw, no trailing newline)**

In `adapters/reasonix/hooks/hook.sh`, line 106, change `jq -r` to `jq -j`:

```bash
        echo "${EVENT}" | jq -j '.toolArgs.content // ""' | run_hector "${FILE}"
```

- [ ] **Step 4: Fix edit_file — preserve trailing newlines through command substitution**

Replace the `PROPOSED=$( … ) || exit 2` block and the `printf` line (lines ~119-144) with the append-sentinel idiom (the python body is unchanged):

```bash
        PROPOSED=$(
          HECTOR_FILE="${FILE}" \
          HECTOR_SEARCH="${SEARCH}" \
          HECTOR_REPLACE="${REPLACE}" \
          python3 -c '
import os, sys
path = os.environ["HECTOR_FILE"]
search = os.environ["HECTOR_SEARCH"]
replace = os.environ.get("HECTOR_REPLACE", "")
try:
    with open(path, "r", encoding="utf-8") as f:
        content = f.read()
except OSError as e:
    print(f"hector: cannot read {path}: {e}", file=sys.stderr)
    sys.exit(2)
count = content.count(search)
if count != 1:
    print(
        f"hector: refusing edit_file — search string appears {count} times in {path}; "
        "Reasonix requires exactly one match",
        file=sys.stderr,
    )
    sys.exit(2)
sys.stdout.write(content.replace(search, replace, 1))
' && printf 'X'
        ) || exit 2
        # $(...) strips trailing newlines; the sentinel 'X' (appended only on
        # python success via &&) preserves them. Strip the sentinel to recover
        # byte-exact content including any trailing newline.
        PROPOSED=${PROPOSED%X}
        printf '%s' "${PROPOSED}" | run_hector "${FILE}"
```

- [ ] **Step 5: Run to verify both tests pass**

Run: `cargo test -p hector-cli --test adapter_reasonix`
Expected: PASS — captured bytes equal the expected content exactly for both write_file and edit_file.

- [ ] **Step 6: Commit**

```bash
git add adapters/reasonix/hooks/hook.sh crates/hector-cli/tests/adapter_reasonix.rs
git commit -m "fix(reasonix): pipe byte-exact proposed content (preserve trailing newline)

jq -j (write_file) and an append-sentinel around \$() (edit_file) stop the hook
mangling trailing newlines — latent until the script engine started reading
piped content. Golden tests assert byte-exact stdin.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Phase 3 — Make it reach users + stay honest

### Task 5: scaffold stdin-form linter rules in `hector init`

**Files:**
- Modify: `crates/hector-cli/src/commands/init/mod.rs` (biome ~113-117, eslint ~128, ruff ~150, and the grep example rules ~92/135/158)
- Test: `crates/hector-cli/src/commands/init/mod.rs` tests (or `init/detect.rs` test module — match where init's generation tests live)

**Why:** init currently emits disk-reading `{file}` rules, so a freshly-init'd project still under-gates pre-write even after Phase 1. Scaffold the stdin forms.

- [ ] **Step 1: Write the failing test**

Locate the existing init-generation test module (search `fn ` + `biome` in `init/mod.rs` / `init/detect.rs`). Add a test asserting the generated biome rule uses the stdin form:

```rust
#[test]
fn scaffolded_biome_rule_uses_stdin_form() {
    let linters = Linters { biome: true, ..Default::default() }; // match the real ctor
    let yaml = render_rules(&linters /* match the real signature */);
    assert!(
        yaml.contains("--stdin-file-path"),
        "biome rule must read stdin so pre-write gating works: {yaml}"
    );
    assert!(!yaml.contains("biome check --no-errors-on-unmatched"),
        "old disk-reading form must be gone: {yaml}");
}
```

(Adjust `Linters`/`render_rules` to the actual names in `init/mod.rs`.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p hector-cli scaffolded_biome_rule_uses_stdin_form`
Expected: FAIL — current template is `biome check --no-errors-on-unmatched {file}`.

- [ ] **Step 3: Switch the linter templates to stdin forms**

In `crates/hector-cli/src/commands/init/mod.rs`:

- biome (line ~117): `"{} biome check --no-errors-on-unmatched {{file}}"` → `"{} biome check --stdin-file-path={{file}}"`
- eslint (line ~128): `"{} eslint --no-error-on-unmatched-pattern {{file}}"` → `"{} eslint --stdin --stdin-filename {{file}}"`
- ruff (line ~150): `"  ruff-check:\n … script: \"ruff check --quiet {{file}}\""` → `… script: \"ruff check --quiet --stdin-filename {{file}} -\""`

For the grep example rules (no-unwrap ~92, no-console-log ~135, no-fixme ~158): drop the `{{file}}` argument so grep reads stdin (the content). E.g. no-console-log (line ~135) becomes:
```
script: "grep -nE 'console\\.log\\(' ; case $? in 0) exit 1;; 1) exit 0;; *) exit $?;; esac"
```
(Apply the same `{{file}}`-removal to no-unwrap and no-fixme. The `grep -n` line numbers are still correct against the piped content.)

- [ ] **Step 4: Run to verify pass + re-trust note**

Run: `cargo test -p hector-cli` (init tests + the new one).
Expected: PASS. If any init snapshot/golden test asserts the old rule text, update it intentionally (`cargo insta review` if `insta` is used) — these are expected, intended changes.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-cli/src/commands/init/mod.rs
git commit -m "feat(init): scaffold stdin-form linter rules

biome --stdin-file-path, eslint --stdin, ruff --stdin-filename, and stdin grep
so a freshly-init'd project gates the proposed edit pre-write instead of
reading the on-disk file.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: documentation honesty — limitation resolved, boundary recorded

**Files:**
- Modify: `crates/hector-cli/src/cli.rs:34-39` (the `--content` help text)
- Modify: `adapters/reasonix/README.md` (the "Limitation: engine: script rules" section)
- Modify: `specs/2026-05-25-reasonix-adapter.md` (§5A known-limitation note, lines ~114-119)
- Modify: `specs/2026-05-29-script-engine-prewrite-content.md` (mark the brief resolved by this plan)

- [ ] **Step 1: Update the `--content` help text**

In `crates/hector-cli/src/cli.rs`, replace the "Limitation" paragraph (lines ~34-39) with the new contract:

```rust
        /// `engine: script` rules receive this content on the command's
        /// **stdin**. Write your tool's stdin form so it gates the proposed
        /// edit pre-write — e.g. `biome check --stdin-file-path={file}`,
        /// `ruff check --stdin-filename {file} -`, `eslint --stdin
        /// --stdin-filename {file}`. A path-only command (`biome check
        /// {file}`) still reads the on-disk file. Whole-program tools (tsc,
        /// cargo, test runners) can't gate a single proposed file — run those
        /// post-write / in CI. AST, semantic, and `hector-disable:` directives
        /// already read `--content`.
```

- [ ] **Step 2: Update the Reasonix README + spec notes**

In `adapters/reasonix/README.md`, rewrite the "Limitation: engine: script rules" section: script rules **do** gate pre-write when written in stdin form; the residual boundary is per-tool (stdin-capable single-file tools work pre-write; whole-program tools belong post-write/CI). Mirror the same correction in `specs/2026-05-25-reasonix-adapter.md` §5A (note the limitation is resolved by `specs/2026-05-29-script-engine-prewrite-content.md` + this plan), and add a one-line "Resolved (see plan `docs/superpowers/plans/2026-05-29-script-engine-prewrite-content.md`)" banner near the top of `specs/2026-05-29-script-engine-prewrite-content.md`.

- [ ] **Step 3: Verify the binary's help renders and commit**

Run: `cargo run -p hector-cli -- check --help` — confirm the new `--content` text renders without panics.

```bash
git add crates/hector-cli/src/cli.rs adapters/reasonix/README.md specs/2026-05-25-reasonix-adapter.md specs/2026-05-29-script-engine-prewrite-content.md
git commit -m "docs: script rules gate pre-write via stdin; record the per-tool boundary

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7 (OPTIONAL — fast-follow): `--explain` advisory for `{file}`-form script rules

**Files:**
- Modify: `crates/hector-core/src/runner.rs` (`RuleExplain` struct ~103-107; the row built in `evaluate_one_rule` ~797; the other `RuleExplain` literals at ~698 and ~1082)
- Modify: `crates/hector-cli/src/commands/check.rs` (`print_explain` ~240-256)

**Why:** runtime detectability of under-gating. When `--content` is authoritative and a `script` rule's command references `{file}` (so it *may* be reading disk rather than stdin), surface a one-line note in the `--explain` report. Advisory, not an assertion — `--stdin-file-path={file}` also references `{file}` and is correct, so phrasing must not claim the rule is wrong.

- [ ] **Step 1: Write the failing test**

Add a `hector-cli` integration test (new file `crates/hector-cli/tests/cli_explain_script_stdin_hint.rs`) that runs `hector check --file foo.txt --content - --explain` with a `{file}`-form script rule and asserts stderr contains a hint substring like `reads stdin only if your command consumes it`.

```rust
// (Use the assert_cmd + write_trusted idioms from cli_check_content.rs.)
// Pipe clean content; rule: script "! grep -q FORBIDDEN {file}".
// Assert: stderr contains "stdin"-hint substring; stdout JSON still valid.
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p hector-cli --test cli_explain_script_stdin_hint`
Expected: FAIL — no such note emitted yet.

- [ ] **Step 3: Add an optional `note` to `RuleExplain` and populate it**

In `crates/hector-core/src/runner.rs`, add the field (struct ~103-107):
```rust
pub struct RuleExplain {
    pub rule_id: String,
    pub engine: EngineKind,
    pub outcome: ExplainOutcome,
    /// Advisory hint surfaced under `--explain` (e.g. a script rule that may be
    /// reading the on-disk file instead of the piped `--content`). Not part of
    /// the verdict JSON.
    pub note: Option<String>,
}
```

Set `note: None` at the two non-dispatch `RuleExplain` literals (~698, ~1082). At the dispatch-path literal (~797), compute it:
```rust
let note = (rule.engine == EngineKind::Script
    && inputs.content_authoritative
    && rule.script.as_deref().is_some_and(|s| s.contains("{file}")))
    .then(|| {
        "script references {file}: reads stdin only if your command consumes it \
         (e.g. --stdin-file-path={file}); a path-only command sees on-disk content"
            .to_string()
    });
```
(Confirm `inputs.content_authoritative` is reachable here; if the field isn't threaded into `evaluate_one_rule`'s `inputs`, thread it the same way `content` is. If it isn't readily available, gate on `inputs.content.is_some()` instead — pre-write/file-mode always supplies content.)

- [ ] **Step 4: Render the note in `print_explain`**

In `crates/hector-cli/src/commands/check.rs`, after the `eprintln!("{} {} {}", …)` (line ~254), add:
```rust
        if let Some(note) = &row.note {
            eprintln!("    note: {note}");
        }
```

- [ ] **Step 5: Run tests + clippy, then commit**

Run: `cargo test -p hector-cli --test cli_explain_script_stdin_hint` (PASS) and `cargo test -p hector-core` (no regressions from the struct change) and `cargo clippy --all-targets -- -D warnings`.

```bash
git add crates/hector-core/src/runner.rs crates/hector-cli/src/commands/check.rs crates/hector-cli/tests/cli_explain_script_stdin_hint.rs
git commit -m "feat(explain): hint when a script rule may read disk under --content

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `cargo test` (full workspace) — all green.
- [ ] `cargo clippy --all-targets -- -D warnings` — clean (watch cognitive complexity on the spawn fns).
- [ ] `cargo fmt --check` — clean.
- [ ] Coverage: in CI, confirm `scripts/ci-coverage.sh` keeps `capability.rs` and `script.rs` ≥80% region (the new tests should cover the stdin branches; if a branch is uncovered — e.g. the timeout-with-writer path — add a targeted test).
- [ ] Manual smoke (the "so what"): in a scratch dir, a stdin-form biome/grep rule + `hector check --file X --content -` blocks forbidden proposed content and passes clean proposed content, regardless of on-disk state.
- [ ] `git status` — only this plan's files changed; the unrelated claude-code `--diff` WIP is untouched.

## Spec coverage self-check (author's review against §9 + §9.1)

- Option A mechanism (offer content on stdin) → Tasks 1, 2, 3. ✓
- Writer correctness (EOF-on-close, EPIPE-tolerant, detach-on-timeout, SIGPIPE reliance) → Task 1 `spawn_stdin_writer` + both spawn paths. ✓
- Linux clone-path + CI-only caveat → Task 3. ✓
- Refinement 1 (detectable boundary, not just documented) → Task 5 (init nudge), Task 6 (help/docs), Task 7 (runtime `--explain` note). ✓
- Refinement 2 (priced choice / honest boundary docs) → Task 6 (README/spec). ✓
- Refinement 3 (Option A activates dormant adapter newline bugs; ship fidelity tests) → Task 4. ✓
- §4 regression → existing seed tests, green at Task 2. ✓
- Deferred (YAGNI): hoist edit→content synthesis into core — explicitly out of scope.
