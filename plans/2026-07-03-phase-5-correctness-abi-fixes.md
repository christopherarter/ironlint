# Phase 5 Correctness / ABI Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three locked ABI / correctness gaps in ironlint 0.4: pre-commit `$IRONLINT_FILES` must be absolute, the unified-diff parser must handle git renames and quoted paths, and `$IRONLINT_FILES` must preserve non-UTF8 path bytes.

**Architecture:** Each fix is a narrow, self-contained change in the core crate. 5.9 absolutizes paths inside the pre-commit dispatch before they reach `GateEnv.files`; 5.10 extends the diff parser's state machine with `rename to/from` handling, C-string unquoting, and a hard error on malformed `+++` headers; 5.11 replaces the lossy `display()` join with `OsStr` concatenation and updates `build_check_env` to accept an `&OsStr`. All three are driven by failing tests first.

**Tech Stack:** Rust, `cargo test`, `cargo clippy`, `cargo fmt`, existing workspace deps (`anyhow`, `tempfile`). No new external dependencies.

## Global Constraints

- Bug fixes start with a failing test.
- Rust files under `crates/*/src/` must meet ≥80% region coverage (run `bash scripts/ci-coverage.sh`).
- Cognitive complexity per function is capped at 15 via clippy (`clippy.toml`); refactor over annotate.
- `Verdict` JSON / `SCHEMA_VERSION` is NOT changed by this work (only item 5.18 bumps it).
- `IRONLINT_*` ABI is a locked stability surface; changes here must be faithful to the documented contract.
- Commit frequently; end with `cargo test` and `cargo clippy --all-targets -- -D warnings`.

---

## File Structure

| File | Role |
|------|------|
| `crates/ironlint-core/src/runner.rs` | Pre-commit dispatch (`check_set`), path resolution, scope matching. Receives 5.9 fix. |
| `crates/ironlint-core/src/diff/parser.rs` | Unified-diff state machine. Receives 5.10 fix (rename + quoted paths + hard malformed-header error). |
| `crates/ironlint-core/src/engine/gate.rs` | `run_gate` and `build_check_env`. Receives 5.11 fix (OsStr join for `$IRONLINT_FILES`). |
| `crates/ironlint-core/tests/diff_parse.rs` | Diff-parser integration tests. New tests for 5.10 go here. |
| `crates/ironlint-core/src/runner.rs` (unit tests) | New tests for 5.9 go in the existing `#[cfg(test)]` module. |
| `crates/ironlint-core/src/engine/gate.rs` (unit tests) | New tests for 5.11 go in the existing `#[cfg(test)]` module. |

---

## Task 1: 5.9 — Pre-commit `$IRONLINT_FILES` must be absolute

**Files:**
- Modify: `crates/ironlint-core/src/runner.rs:771-823` (`check_set`)
- Test: `crates/ironlint-core/src/runner.rs` (append in the existing `#[cfg(test)]` module)

**Interfaces:**
- Consumes: `IronLintEngine::check_set(&self, files: &[PathBuf])` public API (unchanged signature).
- Produces: `GateEnv.files` contains absolute paths for every pre-commit invocation.

- [ ] **Step 1: Write the failing test**

Add this test immediately after `ironlint_file_is_absolute_for_checks` in `crates/ironlint-core/src/runner.rs`:

```rust
#[test]
fn ironlint_files_are_absolute_for_pre_commit_set() {
    // ABI lock: `$IRONLINT_FILES` handed to a pre-commit check is always
    // newline-joined absolute paths. The check blocks (exit 2) iff any
    // entry in `$IRONLINT_FILES` is not absolute.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".ironlint.yml",
        "checks:\n  abs:\n    files: \"**/*.rs\"\n    on: [pre-commit]\n    run: \"for p in \\\"$IRONLINT_FILES\\\"; do case \\\"$p\\\" in /*) ;; *) exit 2;; esac; done\"\n",
    );
    touch(&dir, "a.rs");
    touch(&dir, "b.rs");
    let engine = load_with_event(&dir, "pre-commit");
    // Pass RELATIVE paths into check_set, simulating the CLI --diff path.
    let v = engine.check_set(&[PathBuf::from("a.rs"), PathBuf::from("b.rs")]).unwrap();
    assert_eq!(
        v.status,
        Status::Pass,
        "$IRONLINT_FILES entries must be absolute (check blocks on a non-absolute path): {:?}",
        v.blocks
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
cargo test -p ironlint-core ironlint_files_are_absolute_for_pre_commit_set -- --nocapture
```

Expected: FAIL — `v.status` is `Block` because `$IRONLINT_FILES` contains `a.rs\nb.rs`.

- [ ] **Step 3: Add a path-absolutization helper and use it in `check_set`**

In `crates/ironlint-core/src/runner.rs`, inside `impl IronLintEngine`, add a small private helper near `resolve_input_path`:

```rust
/// Make a path absolute relative to the project root for ABI env vars.
/// Absolute paths pass through; relative paths join onto `config_dir`.
fn absolutize_for_env(&self, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        self.config_dir.join(p)
    }
}
```

Then in `check_set`, change the `matched` collection (around line 785) from:

```rust
let matched: Vec<PathBuf> = files
    .iter()
    .filter(|f| self.check_matches_path(check_id, f))
    .cloned()
    .collect();
```

to:

```rust
let matched: Vec<PathBuf> = files
    .iter()
    .filter(|f| self.check_matches_path(check_id, f))
    .map(|f| self.absolutize_for_env(f))
    .collect();
```

This keeps matching logic unchanged (relative paths still match correctly) but ensures every path placed in `GateEnv.files` is absolute.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p ironlint-core ironlint_files_are_absolute_for_pre_commit_set -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-core/src/runner.rs
git commit -m "fix(core): absolutize pre-commit $IRONLINT_FILES entries

The ABI promises newline-joined absolute paths, but check_set passed
relative paths through unchanged when the caller supplied them (e.g.
CLI --diff). Absolutize against config_dir before building GateEnv.files.

Adds a regression test that feeds relative paths into check_set and
asserts the check sees only absolute paths."
```

---

## Task 2: 5.10 — Diff parser: handle renames and git-quoted paths

**Files:**
- Modify: `crates/ironlint-core/src/diff/parser.rs`
- Test: `crates/ironlint-core/tests/diff_parse.rs`

**Interfaces:**
- Consumes: `parse_unified(input: &str) -> Result<Vec<ChangedFile>>` signature is unchanged.
- Produces: `ChangedFile.op` gains a new `Renamed` variant; `ChangedFile.path` is unquoted for git C-quoted headers; malformed `+++` headers now return `Err`.

- [ ] **Step 1: Write the failing tests**

Append three tests to `crates/ironlint-core/tests/diff_parse.rs`:

```rust
/// A pure `git mv` diff (no ---/+++ pair, only rename from/to headers)
/// must surface the renamed file in the changed set.
#[test]
fn parse_unified_recognizes_rename() {
    use ironlint_core::diff::parser::{ChangeOp, ChangedFile};
    use std::path::PathBuf;

    let input = "diff --git a/old.rs b/new.rs\n\
        similarity index 100%\n\
        rename from old.rs\n\
        rename to new.rs\n";
    let files = ironlint_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0],
        ChangedFile {
            path: PathBuf::from("new.rs"),
            op: ChangeOp::Renamed,
        }
    );
}

/// Paths are C-quoted by git when core.quotePath=true and the path
/// contains non-ASCII bytes. The parser must unquote them.
#[test]
fn parse_unified_unquotes_c_quoted_path() {
    let input = "--- a/caf\\303\\251.rs\n+++ b/caf\\303\\251.rs\n@@ -1,1 +1,1 @@\n-a\n+b\n";
    let files = ironlint_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::PathBuf::from("café.rs"));
}

/// An unrecognized +++ header must fail closed (parse error), not be
/// silently dropped, so a changed file cannot bypass the gate.
#[test]
fn parse_unified_rejects_unrecognized_plus_plus_plus_header() {
    let input = "--- a/foo.rs\n+++ c/foo.rs\n@@ -1,1 +1,1 @@\n-a\n+b\n";
    let err = ironlint_core::diff::parser::parse_unified(input)
        .expect_err("unrecognized +++ header must be a hard parse error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("+++") || msg.contains("unrecognized"),
        "error should mention the malformed +++ header; got: {msg}"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
```bash
cargo test -p ironlint-core --test diff_parse parse_unified_recognizes_rename parse_unified_unquotes_c_quoted_path parse_unified_rejects_unrecognized_plus_plus_plus_header -- --nocapture
```

Expected: FAIL — rename test returns 0 files, quoted-path test yields a path with `\303\251` literal escapes, malformed-header test passes (silently ignored today, not an error).

- [ ] **Step 3: Add the `Renamed` variant and a C-unquote helper**

In `crates/ironlint-core/src/diff/parser.rs`:

1. Add `Renamed` to `ChangeOp`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChangeOp {
    /// The file is new — `--- /dev/null` + `+++ b/<path>`.
    Added,
    /// The file exists in both tree-sides — `--- a/<path>` + `+++ b/<path>`.
    Modified,
    /// The file was removed — `--- a/<path>` + `+++ /dev/null`.
    Deleted,
    /// The file was renamed — `rename from <old>` + `rename to <new>`.
    Renamed,
}
```

2. Add a private C-unquote helper above `parse_unified`:

```rust
/// Undo git's C-style quoting (`core.quotePath`). Git wraps paths in `"`
/// and escapes non-ASCII bytes as `\NNN` octal. We also handle the
/// standard C escapes `\n`, `\t`, `\r`, `\\`, `\"`. Returns the unquoted
/// UTF-8 string; returns an error if the quoted bytes are not valid UTF-8.
fn unquote_git_path(s: &str) -> Result<String> {
    if s.len() < 2 || !s.starts_with('"') || !s.ends_with('"') {
        return Ok(s.to_string());
    }
    let inner = &s[1..s.len() - 1];
    let mut out = Vec::new();
    let mut chars = inner.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\\' {
            // Non-escape characters are added as UTF-8 bytes. In a git-quoted
            // context, printable ASCII passes through literally.
            out.extend_from_slice(c.to_string().as_bytes());
            continue;
        }
        match chars.next() {
            None => return Err(anyhow!("truncated escape in quoted diff path")),
            Some('n') => out.push(b'\n'),
            Some('t') => out.push(b'\t'),
            Some('r') => out.push(b'\r'),
            Some('\\') => out.push(b'\\'),
            Some('"') => out.push(b'"'),
            Some(d) if d.is_ascii_digit() => {
                let mut octal = String::new();
                octal.push(d);
                for _ in 0..2 {
                    if let Some(&next) = chars.peek() {
                        if next.is_ascii_digit() {
                            octal.push(next);
                            chars.next();
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                let byte = u8::from_str_radix(&octal, 8)
                    .map_err(|e| anyhow!("invalid octal escape \\{octal} in quoted diff path: {e}"))?;
                out.push(byte);
            }
            Some(other) => return Err(anyhow!("unsupported escape \\{other} in quoted diff path")),
        }
    }

    String::from_utf8(out).map_err(|e| anyhow!("invalid UTF-8 in quoted diff path: {e}"))
}
```

3. Update `parse_unified` to:
   - unquote paths before validation,
   - handle `rename to` headers,
   - error on unrecognized `+++` headers.

Replace the existing loop body in `parse_unified` with:

```rust
    for raw in input.lines() {
        if let Some(minus) = raw.strip_prefix("--- ") {
            // Flush any in-progress file before starting a new header pair.
            if let Some(f) = current.take() {
                files.push(f);
            }
            let minus = minus.split('\t').next().unwrap_or(minus);
            let minus = minus.trim_end_matches('\r');
            pending_minus = if let Some(path_str) = minus.strip_prefix("a/") {
                let path_str = unquote_git_path(path_str)?;
                validate_path(&path_str)?;
                Some(PathBuf::from(path_str))
            } else {
                None
            };
        } else if let Some(plus) = raw.strip_prefix("+++ ") {
            let plus = plus.split('\t').next().unwrap_or(plus);
            let plus = plus.trim_end_matches('\r');

            if plus == "/dev/null" {
                if let Some(p) = pending_minus.take() {
                    current = Some(ChangedFile {
                        path: p,
                        op: ChangeOp::Deleted,
                    });
                }
            } else if let Some(p) = plus.strip_prefix("b/") {
                let p = unquote_git_path(p)?;
                validate_path(&p)?;
                let pb = PathBuf::from(p);
                let op = if pending_minus.take().is_some() {
                    ChangeOp::Modified
                } else {
                    ChangeOp::Added
                };
                current = Some(ChangedFile { path: pb, op });
            } else {
                return Err(anyhow!("unrecognized +++ header in diff: {plus}"));
            }
        } else if let Some(rename_to) = raw.strip_prefix("rename to ") {
            // Flush any in-progress file; a rename section starts a new entry.
            if let Some(f) = current.take() {
                files.push(f);
            }
            let rename_to = rename_to.split('\t').next().unwrap_or(rename_to);
            let rename_to = rename_to.trim_end_matches('\r');
            let rename_to = unquote_git_path(rename_to)?;
            validate_path(&rename_to)?;
            current = Some(ChangedFile {
                path: PathBuf::from(rename_to),
                op: ChangeOp::Renamed,
            });
            pending_minus = None;
        }
        // All other lines (@@ headers, content, "rename from", etc.) are ignored.
    }
```

4. Update the `run_diff` consumer in `crates/ironlint-cli/src/commands/check.rs` so that `Renamed` files are checked (they exist in the post-change tree). The existing filter:

```rust
        .filter(|f| f.op != ironlint_core::diff::ChangeOp::Deleted)
```

already includes `Renamed` because it only excludes `Deleted`. No change required, but verify by reading the line.

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
cargo test -p ironlint-core --test diff_parse
```

Expected: all diff tests pass, including the three new ones.

Then run:
```bash
cargo test -p ironlint-cli --test cli_e2e_gates
```

Expected: PASS — existing `--diff` behavior is preserved.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-core/src/diff/parser.rs crates/ironlint-core/tests/diff_parse.rs
git commit -m "fix(core): parse renames and git-quoted paths in diffs

- Adds ChangeOp::Renamed for pure git-mv diffs (rename to header).
- Unquotes C-quoted paths produced by core.quotePath=true.
- Turns unrecognized +++ headers into hard parse errors instead of
  silently dropping the changed file.

Adds regression tests for rename detection, non-ASCII unquoting, and
malformed-header rejection."
```

---

## Task 3: 5.11 — `$IRONLINT_FILES` join: use OsStr bytes, not lossy `display()`

**Files:**
- Modify: `crates/ironlint-core/src/engine/gate.rs:72-77` and `:210-243` (`run_gate`, `build_check_env`)
- Test: `crates/ironlint-core/src/engine/gate.rs` (unit tests)

**Interfaces:**
- Consumes: `GateEnv.files: &[PathBuf]` (unchanged).
- Produces: `IRONLINT_FILES` env var is built by concatenating raw `OsStr` bytes, preserving non-UTF8 filenames.

- [ ] **Step 1: Write the failing test**

Add this test in the `#[cfg(test)]` module of `crates/ironlint-core/src/engine/gate.rs`, near the other env tests:

```rust
#[cfg(unix)]
#[test]
fn ironlint_files_preserves_non_utf8_path_bytes() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let dir = tempfile::tempdir().unwrap();
    // A filename that is not valid UTF-8: 0xFF byte.
    let bad = std::ffi::OsString::from_vec(vec![0xFF]);
    let bad_path = dir.path().join(&bad);
    let files = vec![bad_path];

    // Gate blocks iff $IRONLINT_FILES contains the exact raw byte 0xFF.
    // If display() -> U+FFFD replacement happened, the check would see
    // 0xEF 0xBF 0xBD and pass, proving the bug.
    let out = run_gate(
        "printf '%s' \"$IRONLINT_FILES\" | od -An -tx1 | grep -q 'ff' && exit 2 || exit 0",
        &env_with_files(dir.path(), &files),
        None,
        t(),
    );
    assert!(
        matches!(out, GateOutcome::Block { .. }),
        "$IRONLINT_FILES must preserve non-UTF8 path bytes; got: {out:?}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
cargo test -p ironlint-core ironlint_files_preserves_non_utf8_path_bytes -- --nocapture
```

Expected: FAIL — the check passes because `display()` replaced `0xFF` with `U+FFFD`.

- [ ] **Step 3: Replace lossy join with OsStr concatenation**

In `crates/ironlint-core/src/engine/gate.rs`:

1. Change `run_gate`:

Replace:

```rust
    let files_str = env
        .files
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
```

with:

```rust
    let mut files_os = OsString::new();
    for (i, p) in env.files.iter().enumerate() {
        if i > 0 {
            files_os.push("\n");
        }
        files_os.push(p.as_os_str());
    }
```

2. Update the call to `build_check_env` from:

```rust
    cmd.envs(build_check_env(
        env,
        &files_str,
        &std::env::vars_os().collect::<Vec<_>>(),
    ));
```

to:

```rust
    cmd.envs(build_check_env(
        env,
        &files_os,
        &std::env::vars_os().collect::<Vec<_>>(),
    ));
```

3. Update `build_check_env` signature and body:

Replace:

```rust
fn build_check_env(
    env: &GateEnv,
    files_str: &str,
    source: &[(OsString, OsString)],
) -> Vec<(OsString, OsString)>
```

with:

```rust
fn build_check_env(
    env: &GateEnv,
    files_str: &std::ffi::OsStr,
    source: &[(OsString, OsString)],
) -> Vec<(OsString, OsString)>
```

And in the function body, replace:

```rust
    out.push((OsString::from("IRONLINT_FILES"), OsString::from(files_str)));
```

with:

```rust
    out.push((
        OsString::from("IRONLINT_FILES"),
        files_str.to_os_string(),
    ));
```

4. Update the existing unit test `build_check_env_scrubs_secrets_and_keeps_allowlist`:

Change:

```rust
        let files_str = "irrelevant-files-str";
```

to:

```rust
        let files_str = std::ffi::OsStr::new("irrelevant-files-str");
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
cargo test -p ironlint-core ironlint_files_preserves_non_utf8_path_bytes -- --nocapture
cargo test -p ironlint-core build_check_env -- --nocapture
```

Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-core/src/engine/gate.rs
git commit -m "fix(core): preserve non-UTF8 path bytes in $IRONLINT_FILES

Replaces the lossy Path::display() join with raw OsStr concatenation so
filenames containing invalid UTF-8 are passed to checks faithfully.

Updates build_check_env to accept &OsStr and adjusts its unit test.
Adds a Unix-only regression test that asserts the raw bytes survive."
```

---

## Verification (run after all three tasks)

- [ ] **Coverage gate**

```bash
bash scripts/ci-coverage.sh
```

Expected: per-file region coverage ≥80% for all touched files (`runner.rs`, `parser.rs`, `gate.rs`).

- [ ] **Lint and format**

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Full core + CLI test suites**

```bash
cargo test -p ironlint-core
cargo test -p ironlint-cli
```

Expected: all pass.

---

## Self-Review

**1. Spec coverage:**
- 5.9: absolutize pre-commit `$IRONLINT_FILES` → Task 1.
- 5.10: renames, quoted paths, hard error on malformed `+++` → Task 2.
- 5.11: OsStr join for non-UTF8 paths → Task 3.

**2. Placeholder scan:** No TBD/ TODO/ "implement later" / vague "add validation" steps. Every step contains concrete code or exact commands.

**3. Type consistency:**
- `check_set(&[PathBuf])` signature unchanged.
- `parse_unified(&str) -> Result<Vec<ChangedFile>>` signature unchanged.
- `run_gate` / `build_check_env` internal signatures change from `&str` to `&OsStr` / `OsString` consistently.
- `ChangeOp::Renamed` is a new enum variant; the only consumer filter (`!= Deleted`) still behaves correctly.

---

## Execution Handoff

**Plan complete and saved to `plans/2026-07-03-phase-5-correctness-abi-fixes.md`.**

Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — I execute tasks in this session using `superpowers:executing-plans`, with checkpoints for review.

Which approach would you like?
