# Gates → Scripts Rename Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the policy-script directory from `.ironlint/gates/` to `.ironlint/scripts/`, fold its files into the trust hash under that new path, and drop the separate "referenced in-repo scripts" hash fold — collapsing the two-source trust surface into one directory.

**Architecture:** The trust hash (`trust.rs::compute_hash`) stops walking `.ironlint/gates/` and starts walking `.ironlint/scripts/`; the `fold_referenced_scripts` pipeline (5 functions) is deleted as dead code. `BlessedSummary` is reshaped: `gates: Vec<String>` → `checks: usize`, and `scripts: Vec<String>` is repurposed from "referenced in-repo scripts" to "files under `.ironlint/scripts/`". The bash-gate's `is_policy_path` matcher swaps `.ironlint/gates/` for `.ironlint/scripts/`. The four adapters (two shell hooks, two TS plugins) short-circuit edits under `.ironlint/scripts/` — path-anchored, not basename, so `src/.ironlint/scripts/foo.sh` is not matched.

**Tech Stack:** Rust (workspace: `ironlint-core`, `ironlint-cli`, `ironlint-bash-gate`), bash adapters, TypeScript adapters (pi, opencode). Tests: `cargo test` (Rust), `bun test` (TS adapters).

## Global Constraints

- This is a **hard rename, no migration path**. Existing projects must move scripts from `.ironlint/gates/` to `.ironlint/scripts/` and re-run `ironlint trust`. There is no install base (tool hasn't shipped) — do not add backward-compat shims.
- **Trust hash surface:** the hash covers (1) the resolved `.ironlint.yml` bytes (entire extends closure, unchanged) and (2) every regular file under `PROJECT_ROOT/.ironlint/scripts/` (sorted by relative path). Arbitrary in-repo scripts referenced by `run:`/`steps[].run` but located **outside** `.ironlint/scripts/` are **no longer folded** — a deliberate simplification per the spec. A check may still reference them, but changing them does not revoke trust.
- **Bash-gate policy surface:** blocks `ironlint trust` (any args) and Bash writes to `.ironlint.yml` or any file under `.ironlint/scripts/`. The `gate_bash.rs` CLI adapter is a pure stdin→exit shim and needs **no changes** — the matcher is `is_policy_path` in `crates/ironlint-bash-gate/src/lib.rs`. (The spec's "Files touched" line naming `gate_bash.rs` is a misnomer; the work is in the bash-gate crate.)
- **Adapter short-circuit is path-anchored, not basename.** Matching `src/.ironlint/scripts/foo.sh` as policy would be a bug. Anchor the `.ironlint/scripts/` check to the project root the adapter already captures (`projectRoot` in both TS plugins; `PROJECT_ROOT`/`CWD` in the shell hooks).
- **≥80% region coverage** per Rust file under `crates/*/src/` (enforced by `scripts/ci-coverage.sh`). Cognitive complexity per function capped at **15** (`clippy.toml`).
- **TDD.** Bug fixes / behavior changes start with a failing test. The failing test becomes regression coverage.
- `Cargo.lock` is committed (workspace policy). After any dependency touch, regenerate with `cargo generate-lockfile` or a plain `cargo build`. This plan touches no dependencies, so no lockfile change is expected.
- Binary is `ironlint`, not `ironlint-cli`.

---

## File Structure

**Modified (Rust):**
- `crates/ironlint-core/src/trust.rs` — rename the gates dir to `.ironlint/scripts/`; delete `fold_referenced_scripts`, `referenced_repo_files`, `candidate_repo_relative`, `is_regular_file_no_follow`, `tokenize_command` (now dead code); reshape `BlessedSummary` (`gates` → `checks: usize`; `scripts` repurposed); update `compute_hash` and `blessed_summary` to walk `.ironlint/scripts/`.
- `crates/ironlint-cli/src/commands/trust.rs` — `render_summary` prints `checks: N` and `scripts: N` (always prints `scripts:` block, since it's now the policy dir, not an optional referenced-set).
- `crates/ironlint-cli/src/commands/doctor.rs` — doc comments + remediation strings + `check_run_path` path prefix (`.ironlint/scripts/`).
- `crates/ironlint-cli/src/cli.rs` — `Trust` subcommand help text (`.ironlint/gates/` → `.ironlint/scripts/`).
- `crates/ironlint-bash-gate/src/lib.rs` — `is_policy_path` matcher: `.ironlint/gates/` → `.ironlint/scripts/`; header doc comment; `cd_into_policy_then_trust` doc comment; all `assert_blocks`/`assert_allows` test strings.

**Modified (adapters):**
- `adapters/claude-code/hooks/hook.sh` — short-circuit edits under `${PROJECT_ROOT}/.ironlint/scripts/`.
- `adapters/codex/hooks/hook.sh` — same short-circuit.
- `adapters/pi/src/index.ts` — `isPolicyFile` becomes path-anchored against `projectRoot` for `.ironlint/scripts/`.
- `adapters/opencode/src/index.ts` — same.

**Modified (tests):**
- `crates/ironlint-core/src/trust.rs` (inline `mod tests`) — path + shape updates.
- `crates/ironlint-cli/tests/cli_e2e_trust.rs` — path + summary-output updates.
- `crates/ironlint-cli/tests/cli_e2e_doctor.rs` — path updates.
- `crates/ironlint-bash-gate/src/lib.rs` (inline tests) — path updates.
- `crates/ironlint-core/tests/trust_extends.rs` — path updates.
- `adapters/pi/test/index.test.ts` — path-anchored policy test.
- `adapters/opencode/tests/plugin.test.ts` — path-anchored policy test.

**Modified (docs):** `docs/architecture.md`, `docs/security/trust.md`, `docs/writing-checks/recipes.md`, `docs/writing-checks/README.md`, `docs/getting-started.md`, `docs/adapters/README.md`, `docs/configuring/targeting-files.md`, `CLAUDE.md` (project instructions), and the bash-gate self-trust spec/plan (`.ironlint/gates/` → `.ironlint/scripts/` references). Docs are a final sweep task, not per-feature.

---

## Task 1: Rename the trust hash directory and drop referenced-scripts fold

**Files:**
- Modify: `crates/ironlint-core/src/trust.rs` (whole file: `closure_gate_dirs`, `compute_hash`, `blessed_summary`, and the five deleted functions)

**Interfaces:**
- Consumes: `crate::config::extends::{resolve, resolve_paths}`, `crate::adapter::sha256_digest_hex`, `crate::config::Config` (only via the deleted `referenced_repo_files` — dropped with it).
- Produces:
  - `pub fn compute_hash(config_path: &Path) -> Result<String>` — unchanged signature; now walks `.ironlint/scripts/` instead of `.ironlint/gates/` and no longer folds referenced scripts. Label namespace changes from `gates\0<dir>\0<rel>` to `scripts\0<dir>\0<rel>`.
  - `pub struct BlessedSummary { config_path: PathBuf, config_hash: String, checks: usize, scripts: Vec<String> }` — `gates` field removed; `checks` added; `scripts` repurposed.
  - `pub fn blessed_summary(config_path: &Path) -> Result<BlessedSummary>` — unchanged signature; `checks` = resolved check count, `scripts` = files under `.ironlint/scripts/`.
  - (Deleted, no longer public API: `fold_referenced_scripts`, `referenced_repo_files`, `candidate_repo_relative`, `is_regular_file_no_follow`, `tokenize_command`. None are referenced outside `trust.rs` — verify with `grep -rn fold_referenced_scripts\|referenced_repo_files\|tokenize_command\|candidate_repo_relative\|is_regular_file_no_follow crates/` before deleting.)

- [ ] **Step 1: Write the failing tests** (update the existing inline tests in `trust.rs` to assert the new behavior).

Replace the test bodies that pin the old behavior. In `crates/ironlint-core/src/trust.rs`, first update the three tests that reference `.ironlint/gates/` paths so they expect `.ironlint/scripts/`:

For `editing_a_gate_script_changes_hash` (around line 768):

```rust
    #[test]
    fn editing_a_script_changes_hash() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \".ironlint/scripts/g.sh\"\n",
        );
        let script = dir.path().join(".ironlint/scripts/g.sh");
        write(&script, "#!/bin/sh\nexit 0\n");
        let before = compute_hash(&cfg).unwrap();
        write(&script, "#!/bin/sh\nexit 2\n");
        let after = compute_hash(&cfg).unwrap();
        assert_ne!(before, after, "a script edit must invalidate the hash");
    }
```

For `hash_folds_gate_files_in_sorted_order` (around line 785) — rename to `hash_folds_scripts_in_sorted_order` and update the label namespace:

```rust
    #[test]
    fn hash_folds_scripts_in_sorted_order() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        let cfg_body = "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n";
        write(&cfg, cfg_body);
        write(&dir.path().join(".ironlint/scripts/a.sh"), "a\n");
        write(&dir.path().join(".ironlint/scripts/b.sh"), "b\n");

        let canon = cfg.canonicalize().unwrap();
        let scripts_dir = canon.parent().unwrap().join(".ironlint").join("scripts");

        let mut expected = Sha256::new();
        hash_entry(
            &mut expected,
            &format!("config\0{}", canon.display()),
            cfg_body.as_bytes(),
        );
        hash_entry(
            &mut expected,
            &format!("scripts\0{}\0a.sh", scripts_dir.display()),
            b"a\n",
        );
        hash_entry(
            &mut expected,
            &format!("scripts\0{}\0b.sh", scripts_dir.display()),
            b"b\n",
        );
        let want = sha256_digest_hex(&expected.finalize());

        assert_eq!(compute_hash(&cfg).unwrap(), want);
    }
```

For `cfg_with_gate` helper (around line 1214):

```rust
    fn cfg_with_script(dir: &Path) -> PathBuf {
        let cfg = dir.join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*\"\n    run: \".ironlint/scripts/g.sh\"\n",
        );
        write(&dir.join(".ironlint/scripts/g.sh"), "#!/bin/sh\nexit 0\n");
        cfg
    }
```

Then update every call site of `cfg_with_gate` to `cfg_with_script` (there are several in the bless/ensure tests below it). Finally, flip the now-reversed referenced-scripts tests to assert the **new** behavior — that editing a referenced script outside `.ironlint/scripts/` does **not** change the hash. For `editing_a_referenced_script_changes_hash` (around line 834), rename and invert:

```rust
    #[test]
    fn editing_a_referenced_outside_script_does_not_change_hash() {
        // After the gates→scripts rename: a script referenced by `run:` but
        // located OUTSIDE .ironlint/scripts/ is no longer folded into the
        // trust hash. It may still be run by a check, but changing it does
        // not revoke trust — the spec's deliberate simplification.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*\"\n    run: \"bash scripts/lint.sh\"\n",
        );
        let script = dir.path().join("scripts/lint.sh");
        write(&script, "#!/bin/sh\nexit 0\n");
        let before = compute_hash(&cfg).unwrap();
        write(&script, "#!/bin/sh\nexit 2\n");
        let after = compute_hash(&cfg).unwrap();
        assert_eq!(
            before, after,
            "editing an in-repo script OUTSIDE .ironlint/scripts/ no longer revokes trust"
        );
    }
```

Delete these now-obsolete tests entirely (their invariants are gone): `editing_an_unrelated_file_does_not_change_hash`, `referenced_script_in_steps_run_changes_hash`, `referenced_script_from_inherited_check_uses_primary_root`, `referenced_out_of_repo_path_is_not_folded`, `referenced_script_symlink_is_skipped_not_followed`, `editing_a_referenced_script_after_bless_revokes_trust`. Remove the `BTreeSet` import if it becomes unused (it does — `referenced_repo_files` was the only consumer; the `use std::collections::{BTreeMap, BTreeSet};` at the top of file becomes `use std::collections::BTreeMap;`). Update `gate_files_recurse_into_subdirectories` → `scripts_recurse_into_subdirectories`, `gates_dir_symlink_loop_is_a_clear_error_not_a_hang` → `scripts_dir_symlink_loop_is_a_clear_error_not_a_hang`, `gates_dir_itself_as_symlink_is_refused` → `scripts_dir_itself_as_symlink_is_refused`, `gates_dir_path_is_a_plain_file_is_an_error` → `scripts_dir_path_is_a_plain_file_is_an_error`, `missing_gates_dir_hashes_only_the_config` → `missing_scripts_dir_hashes_only_the_config` — each swapping `.ironlint/gates/` for `.ironlint/scripts/` in its setup. Update the `blessed_summary_lists_hash_gates_and_scripts` test (around line 1231) to the new shape:

```rust
    #[test]
    fn blessed_summary_lists_hash_checks_and_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*\"\n    run: \"bash scripts/lint.sh\"\n",
        );
        write(&dir.path().join(".ironlint/scripts/a.sh"), "a\n");
        write(&dir.path().join(".ironlint/scripts/b.sh"), "b\n");
        write(&dir.path().join("scripts/lint.sh"), "#!/bin/sh\nexit 0\n");

        let summary = blessed_summary(&cfg).unwrap();

        assert!(
            summary.config_hash.starts_with("sha256:"),
            "config_hash must be sha256-prefixed: {}",
            summary.config_hash
        );
        assert_eq!(
            summary.config_hash,
            compute_hash(&cfg).unwrap(),
            "blessed_summary must report the SAME digest compute_hash would produce"
        );
        assert_eq!(
            summary.checks, 1,
            "checks counts resolved checks"
        );
        assert_eq!(
            summary.scripts,
            vec!["a.sh".to_string(), "b.sh".to_string()],
            "scripts lists every file under .ironlint/scripts/, sorted"
        );
    }
```

And `blessed_summary_is_empty_with_no_gates_or_scripts` → `blessed_summary_is_empty_with_no_scripts`:

```rust
    #[test]
    fn blessed_summary_is_empty_with_no_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".ironlint.yml");
        write(
            &cfg,
            "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
        );

        let summary = blessed_summary(&cfg).unwrap();

        assert!(summary.scripts.is_empty(), "no scripts dir → empty scripts list");
        assert_eq!(summary.checks, 1, "the one inline check still counts");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p ironlint-core --lib trust
```
Expected: FAIL — the old code still walks `.ironlint/gates/` and keeps the `gates` field; the renamed/shape-changed tests won't compile or won't match. (Some fail to compile because `BlessedSummary.gates` is gone.)

- [ ] **Step 3: Implement the rename.** In `crates/ironlint-core/src/trust.rs`:

  1. **Delete** the five now-dead functions: `tokenize_command` (≈ line 127), `is_regular_file_no_follow` (≈ 145), `candidate_repo_relative` (≈ 159), `referenced_repo_files` (≈ 203), `fold_referenced_scripts` (≈ 240).
  2. **`closure_gate_dirs` → `closure_script_dirs`**: change `.join("gates")` to `.join("scripts")`:
     ```rust
     fn closure_script_dirs(config_paths: &[PathBuf]) -> Vec<PathBuf> {
         let mut script_dirs: Vec<PathBuf> = config_paths
             .iter()
             .map(|p| {
                 p.parent()
                     .unwrap_or_else(|| Path::new("."))
                     .join(".ironlint")
                     .join("scripts")
             })
             .collect();
         script_dirs.sort();
         script_dirs.dedup();
         script_dirs
     }
     ```
  3. **`compute_hash`**: rename `gate_dirs` → `script_dirs`, call `closure_script_dirs`, rename the loop var `gates_dir` → `scripts_dir`, change the label from `gates\0` to `scripts\0`, and **delete** the `fold_referenced_scripts(...)` call. Updated body (lines ≈ 295–341):
     ```rust
     pub fn compute_hash(config_path: &Path) -> Result<String> {
         let mut hasher = Sha256::new();

         let config_paths = crate::config::extends::resolve_paths(config_path)
             .with_context(|| format!("resolving extends closure for {}", config_path.display()))?;

         for path in &config_paths {
             let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
             hash_entry(&mut hasher, &format!("config\0{}", path.display()), &bytes);
         }

         let script_dirs = closure_script_dirs(&config_paths);
         for scripts_dir in &script_dirs {
             match classify_entry(scripts_dir)? {
                 EntryKind::Dir => {
                     for (rel, bytes) in collect_gate_files(scripts_dir)? {
                         hash_entry(
                             &mut hasher,
                             &format!("scripts\0{}\0{rel}", scripts_dir.display()),
                             &bytes,
                         );
                     }
                 }
                 EntryKind::Missing => {}
                 EntryKind::File => {
                     anyhow::bail!(
                         "expected {} to be a directory (scripts dir)",
                         scripts_dir.display()
                     );
                 }
             }
         }

         Ok(sha256_digest_hex(&hasher.finalize()))
     }
     ```
     (Keep `collect_gate_files` as-is — its name is internal; renaming it is optional churn. If you do rename it to `collect_script_files`, rename the call sites and the `collect_into` helper too. Prefer leaving it to minimize the diff.)
  4. **`BlessedSummary`**: remove `gates`, add `checks`:
     ```rust
     #[derive(Debug, Clone, PartialEq, Eq)]
     pub struct BlessedSummary {
         pub config_path: PathBuf,
         pub config_hash: String,
         /// Number of resolved checks (post-extends merge).
         pub checks: usize,
         /// Every file relative path under `.ironlint/scripts/`, sorted and deduped.
         pub scripts: Vec<String>,
     }
     ```
  5. **`blessed_summary`** (≈ line 375): replace the gates-collection block and the referenced-scripts block with a scripts walk + a check count. The merged config is already parsed via `crate::config::extends::resolve`; `.checks.len()` is the count:
     ```rust
     pub fn blessed_summary(config_path: &Path) -> Result<BlessedSummary> {
         let config_hash = compute_hash(config_path)?;

         let config_paths = crate::config::extends::resolve_paths(config_path)
             .with_context(|| format!("resolving extends closure for {}", config_path.display()))?;
         let script_dirs = closure_script_dirs(&config_paths);

         let mut scripts: Vec<String> = Vec::new();
         for dir in &script_dirs {
             match classify_entry(dir)? {
                 EntryKind::Dir => {
                     for (rel, _bytes) in collect_gate_files(dir)? {
                         scripts.push(rel);
                     }
                 }
                 EntryKind::Missing => {}
                 EntryKind::File => {
                     anyhow::bail!("expected {} to be a directory (scripts dir)", dir.display());
                 }
             }
         }
         scripts.sort();
         scripts.dedup();

         let merged = crate::config::extends::resolve(config_path)?;
         let checks = merged.checks.len();

         Ok(BlessedSummary {
             config_path: config_path.to_path_buf(),
             config_hash,
             checks,
             scripts,
         })
     }
     ```
  6. Fix the `classify_entry` / `collect_gate_files` error messages that say "gates dir" to "scripts dir" (the `bail!` calls at ≈ lines 53, 63, 111, 325, 392). These are user-facing strings; keeping them consistent with the new directory name matters for the symlink/refusal error clarity.
  7. Update the doc comment on `compute_hash` (≈ line 281) and `blessed_summary` (≈ 343) — replace "gate scripts" / "gates dir" with "scripts under `.ironlint/scripts/`" and drop the "referenced scripts" mentions.
  8. Drop now-unused imports: `BTreeSet` (and `BTreeMap` if nothing else uses it — `TrustStore.entries` is a `BTreeMap`, so keep `BTreeMap`). Confirm `crate::config::Config` is still imported only if used; after deletion, `Config` is no longer referenced — remove it from the `use crate::config::Config;` line (≈ line 2). Run `cargo check -p ironlint-core` to surface dangling imports.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p ironlint-core --lib trust
```
Expected: PASS — all renamed/reshaped tests green.

- [ ] **Step 5: Run clippy + fmt**

```bash
cargo clippy -p ironlint-core --all-targets -- -D warnings
cargo fmt
```
Expected: no warnings (cognitive complexity stays ≤15; the deleted code lowers complexity). If `cargo fmt` rewrites anything, re-run the tests.

- [ ] **Step 6: Commit**

```bash
git add crates/ironlint-core/src/trust.rs
git commit -m "refactor(trust): rename .ironlint/gates/ to .ironlint/scripts/, drop referenced-scripts fold

BlessedSummary.gates → checks (usize); scripts repurposed from referenced
in-repo scripts to files under .ironlint/scripts/. fold_referenced_scripts
and its five helpers are deleted as dead code (deliberate simplification:
a script that is part of the policy surface belongs in .ironlint/scripts/)."
```

---

## Task 2: Update the `ironlint trust` CLI summary rendering

**Files:**
- Modify: `crates/ironlint-cli/src/commands/trust.rs` (`render_summary` + its inline tests)

**Interfaces:**
- Consumes: `ironlint_core::trust::BlessedSummary` (the reshaped struct from Task 1: `{ config_path, config_hash, checks: usize, scripts: Vec<String> }`).
- Produces: `render_summary(&BlessedSummary) -> String` printing the spec's exact block.

- [ ] **Step 1: Write the failing tests.** Replace the inline test module in `crates/ironlint-cli/src/commands/trust.rs`. The `summary()` helper must build the new shape; the assertions pin the spec's output (`checks: N` always; `scripts:` block always printed, listing relative paths):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn summary(checks: usize, scripts: Vec<&str>) -> BlessedSummary {
        BlessedSummary {
            config_path: PathBuf::from("/abs/.ironlint.yml"),
            config_hash: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd"
                .to_string(),
            checks,
            scripts: scripts.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn render_summary_truncates_hash_to_16_hex_chars() {
        let out = render_summary(&summary(0, vec![]));
        assert!(
            out.contains("config sha256: 0123456789abcdef\n"),
            "hash must be truncated to the first 16 hex chars: {out}"
        );
        assert!(
            !out.contains("0123456789abcdef0"),
            "must not print more than 16 hex chars: {out}"
        );
    }

    #[test]
    fn render_summary_prints_checks_and_scripts() {
        let out = render_summary(&summary(6, vec!["lint.sh", "no-todo.sh"]));
        assert!(out.contains("checks: 6"), "must print checks count: {out}");
        assert!(out.contains("scripts: 2"), "must print scripts count: {out}");
        assert!(out.contains("    - lint.sh"));
        assert!(out.contains("    - no-todo.sh"));
    }

    #[test]
    fn render_summary_prints_scripts_block_even_when_empty() {
        // The scripts block is the policy-directory listing — always present,
        // even when empty, so the operator sees "scripts: 0" explicitly rather
        // than wondering whether the surface was omitted.
        let out = render_summary(&summary(2, vec![]));
        assert!(out.contains("checks: 2"));
        assert!(out.contains("scripts: 0"), "scripts: 0 is printed, not omitted: {out}");
    }

    #[test]
    fn render_summary_guards_against_a_short_hash() {
        let mut s = summary(0, vec![]);
        s.config_hash = "sha256:abcd".to_string();
        let out = render_summary(&s);
        assert!(
            out.contains("config sha256: abcd\n"),
            "a short hash must not index-panic, just print what's there: {out}"
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p ironlint-cli --lib commands::trust
```
Expected: FAIL — `render_summary` still references `summary.gates` (compile error after Task 1's reshape) and omits `checks:`.

- [ ] **Step 3: Implement `render_summary`.** Replace the function body in `crates/ironlint-cli/src/commands/trust.rs` (≈ lines 36–59):

```rust
/// Render a [`BlessedSummary`] as the indented, human-facing block printed
/// after the `trusted: <path>` line. Kept separate from `run` (and taking a
/// borrowed summary rather than doing its own I/O) so it is unit-testable
/// without a subprocess.
fn render_summary(summary: &BlessedSummary) -> String {
    let hex = summary
        .config_hash
        .strip_prefix("sha256:")
        .unwrap_or(&summary.config_hash);
    let short_len = hex.len().min(16);

    let mut lines = vec![
        format!("  config sha256: {}", &hex[..short_len]),
        format!("  checks: {}", summary.checks),
        format!("  scripts: {}", summary.scripts.len()),
    ];
    for script in &summary.scripts {
        lines.push(format!("    - {script}"));
    }

    lines.join("\n")
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p ironlint-cli --lib commands::trust
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-cli/src/commands/trust.rs
git commit -m "refactor(trust-cli): render checks + scripts summary for .ironlint/scripts/"
```

---

## Task 3: Update `doctor` for `.ironlint/scripts/`

**Files:**
- Modify: `crates/ironlint-cli/src/commands/doctor.rs` (`check_script_paths` remediation string, `check_run_path` prefix, doc comments)

**Interfaces:**
- Consumes: `ironlint_core::config::parse_file_with_extends` (unchanged), the resolved `Config`'s `checks` map and `effective_steps()`.
- Produces: unchanged public surface (`run`, `CheckResult`, `Report`); only error strings and the `.ironlint/scripts/` path prefix change.

- [ ] **Step 1: Write the failing test.** Update the existing `check_run_path_fails_missing_script` test (≈ line 617) to use the new path:

```rust
    #[test]
    fn check_run_path_fails_missing_script() {
        let d = tempdir().unwrap();
        let result = check_run_path(d.path(), "g", ".ironlint/scripts/missing.sh");
        assert!(result.is_some());
        assert!(result.unwrap().contains("not found"));
    }
```

Add a guard test confirming a `.ironlint/gates/` path is now treated as out-of-scope (not checked):

```rust
    #[test]
    fn check_run_path_skips_legacy_gates_path() {
        // After the rename, doctor only checks scripts under .ironlint/scripts/.
        // A legacy .ironlint/gates/ path is not the policy surface and is skipped
        // (returns None) rather than flagged as missing.
        assert!(check_run_path(Path::new("."), "g", ".ironlint/gates/missing.sh").is_none());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p ironlint-cli --lib commands::doctor
```
Expected: FAIL — `check_run_path` still checks any `.ironlint/` prefix, so the legacy-gates guard test fails (`is_none()` is false).

- [ ] **Step 3: Implement.** In `crates/ironlint-cli/src/commands/doctor.rs`:

  1. Update the doc-comment block at the top (≈ lines 11–15): change `check_scripts` description and the trust row to reference `.ironlint/scripts/`:
     ```
     //!   4. check_scripts — each check whose `run` names a single-token path that
     //!      starts with `.ironlint/scripts/` exists and is executable
     //!   5. trust — config + `.ironlint/scripts/` are blessed in the trust store
     ```
  2. In `check_script_paths`, update the remediation string (≈ line 204):
     ```rust
     remediation: Some(
         "ensure check scripts exist under .ironlint/scripts/ and are executable (chmod +x)"
             .into(),
     ),
     ```
  3. In `check_run_path` (≈ line 220), tighten the prefix from `.ironlint/` to `.ironlint/scripts/`:
     ```rust
     fn check_run_path(dir: &Path, check_id: &str, run: &str) -> Option<String> {
         if run.contains(' ') {
             return None;
         }
         if !run.starts_with(".ironlint/scripts/") {
             return None;
         }
         let script = dir.join(run);
         if !script.exists() {
             return Some(format!("{check_id}: {run} not found"));
         }
         #[cfg(unix)]
         {
             use std::os::unix::fs::PermissionsExt;
             if let Ok(meta) = std::fs::metadata(&script) {
                 if meta.permissions().mode() & 0o111 == 0 {
                     return Some(format!("{check_id}: {run} not executable"));
                 }
             }
         }
         None
     }
     ```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p ironlint-cli --lib commands::doctor
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-cli/src/commands/doctor.rs
git commit -m "refactor(doctor): check scripts under .ironlint/scripts/"
```

---

## Task 4: Update the `gate-bash` matcher to block writes to `.ironlint/scripts/`

**Files:**
- Modify: `crates/ironlint-bash-gate/src/lib.rs` (`is_policy_path`, header doc comment, `cd_into_policy_then_trust` doc comment, all inline tests)
- **Not** modified: `crates/ironlint-cli/src/commands/gate_bash.rs` (pure stdin→exit shim; no path references).

**Interfaces:**
- Consumes: nothing (pure string matcher).
- Produces: `ironlint_bash_gate::decide(command) -> Decision` — unchanged signature; the deny set now covers `.ironlint/scripts/` instead of `.ironlint/gates/`.

- [ ] **Step 1: Write the failing tests.** Update every `.ironlint/gates/` string in the inline tests to `.ironlint/scripts/`. Concretely, in `crates/ironlint-bash-gate/src/lib.rs`:
  - Line 592: `assert_blocks("cd .ironlint/gates && trust")` → `assert_blocks("cd .ironlint/scripts && trust")`
  - Line 741 comment: `--- same detectors against .ironlint/gates/ ---` → `--- same detectors against .ironlint/scripts/ ---`
  - Line 744: `assert_blocks("echo x > .ironlint/gates/lint.sh")` → `.ironlint/scripts/lint.sh`
  - Line 749: `assert_blocks("sed -i 's/x/y/' .ironlint/gates/lint.sh")` → `.ironlint/scripts/lint.sh`
  - Line 765: `assert_blocks("cp malicious.sh .ironlint/gates/lint.sh")` → `.ironlint/scripts/lint.sh`
  - Line 791: `assert_allows("ls .ironlint/gates/")` → `assert_allows("ls .ironlint/scripts/")`
  - Line 796: `assert_allows("cat .ironlint/gates/lint.sh")` → `assert_allows("cat .ironlint/scripts/lint.sh")`
  - Line 1004: `assert_blocks("dd of=.ironlint/gates/x.sh")` → `assert_blocks("dd of=.ironlint/scripts/x.sh")`
  - Rename the affected test functions for clarity: `blocks_redirect_to_gate_script` → `blocks_redirect_to_policy_script`, `blocks_sed_inplace_gate_script` → `blocks_sed_inplace_policy_script`, `blocks_cp_onto_gate_script` → `blocks_cp_onto_policy_script`, `allows_ls_ironlint_gates` → `allows_ls_ironlint_scripts`, `blocks_dd_of_gate_script` → `blocks_dd_of_policy_script`.

Add a **new** test asserting the old `.ironlint/gates/` path is now **allowed** (the rename means it's no longer the policy surface):

```rust
    #[test]
    fn allows_write_to_legacy_gates_path() {
        // After the gates→scripts rename, .ironlint/gates/ is no longer the
        // policy surface — a Bash write there is allowed (it's just a regular
        // repo directory now). Pin this so the matcher doesn't regress.
        assert_allows("echo x > .ironlint/gates/lint.sh");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p ironlint-bash-gate --lib
```
Expected: FAIL — `is_policy_path` still matches `.ironlint/gates/`, so the renamed block-tests fail (matcher allows what the test expects to block) and `allows_write_to_legacy_gates_path` fails (matcher blocks what the test expects to allow).

- [ ] **Step 3: Implement.** In `crates/ironlint-bash-gate/src/lib.rs`:

  1. `is_policy_path` (line 323): change `token.contains(".ironlint/gates/")` → `token.contains(".ironlint/scripts/")`. Updated function:
     ```rust
     fn is_policy_path(token: &str) -> bool {
         token.ends_with(".ironlint.yml")
             || token.contains("/.ironlint.yml")
             || token.contains(".ironlint/scripts/")
     }
     ```
  2. Header doc comment (line 5): `.ironlint/gates/` → `.ironlint/scripts/`.
  3. `cd_into_policy_then_trust` doc comment (≈ lines 279–284): update the `cd .ironlint/gates` example to `cd .ironlint/scripts`.
  4. `is_policy_path` doc comment (≈ lines 306–308): update `.ironlint/gates/` → `.ironlint/scripts/`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p ironlint-bash-gate --lib
cargo clippy -p ironlint-bash-gate --all-targets -- -D warnings
```
Expected: PASS, no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-bash-gate/src/lib.rs
git commit -m "refactor(bash-gate): block writes to .ironlint/scripts/ (was .ironlint/gates/)"
```

---

## Task 5: Update the claude-code and codex shell hooks

**Files:**
- Modify: `adapters/claude-code/hooks/hook.sh`
- Modify: `adapters/codex/hooks/hook.sh`

**Interfaces:**
- Consumes: the event JSON (`tool_name`, `tool_input.file_path`/`path`/`notebook_path`, `cwd`).
- Produces: unchanged exit contract; adds a path-anchored short-circuit for `${PROJECT_ROOT}/.ironlint/scripts/`.

- [ ] **Step 1: Write the failing test.** Add a Rust integration test in a new file `crates/ironlint-cli/tests/hook_contract_scripts_dir.rs` that drives the claude-code hook against a Write event whose `file_path` is under `.ironlint/scripts/` and asserts the hook short-circuits (exit 0) **without** invoking `ironlint check` — i.e., the policy-dir write is allowed through the gate. (The existing `hook_contract_claude_code.rs` already stubs `ironlint`; extend the same fixture pattern.) Use a `Fx`-style stub harness; if the existing `hook_contract_claude_code.rs` is the place these live, add the test there instead of a new file to reuse its fixture. Failing assertion: the hook should NOT call `ironlint check` for a `.ironlint/scripts/foo.sh` write, but the current hook only short-circuits `.ironlint.yml` (basename match at line 126), so it would run the check and hit the trust gate.

```rust
// In crates/ironlint-cli/tests/hook_contract_claude_code.rs (append):
/// After the gates→scripts rename, writes to files under .ironlint/scripts/
/// short-circuit the gate exactly like .ironlint.yml edits — a mid-edit
/// policy script's on-disk bytes won't match the trusted hash, so checking
/// it would surface a misleading "internal error". The short-circuit must
/// be PATH-ANCHORED so src/.ironlint/scripts/foo.sh (not the policy surface)
/// is NOT matched.
#[test]
fn write_to_scripts_dir_short_circuits_without_check() {
    let fx = Fx::new(); // reuse the existing fixture builder
    fx.stub_ironlint(|_args, _stdin| {
        panic!("ironlint check must not be invoked for a .ironlint/scripts/ write");
    });
    let event = serde_json::json!({
        "tool_name": "Write",
        "tool_input": { "file_path": fx.path(".ironlint/scripts/lint.sh"), "content": "x\n" },
    });
    let ec = fx.run_hook(event);
    assert_eq!(ec, 0, "a .ironlint/scripts/ write must short-circuit (allow)");
}
```
(Adapt the `Fx` builder and `stub_ironlint` to match the actual fixture API in `hook_contract_claude_code.rs`. The exact method names may differ — read that file first and mirror an existing passing test's structure. The key invariant under test: the panic-on-check proves the short-circuit fired.)

Add the codex-side mirror in `crates/ironlint-cli/tests/hook_contract_codex.rs` with the same invariant.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p ironlint-cli --test hook_contract_claude_code write_to_scripts_dir
cargo test -p ironlint-cli --test hook_contract_codex write_to_scripts_dir
```
Expected: FAIL — the hooks don't short-circuit `.ironlint/scripts/` yet, so `ironlint check` runs and either the stub panics or the hook exits nonzero.

- [ ] **Step 3: Implement.**

**claude-code hook** (`adapters/claude-code/hooks/hook.sh`): the current basename short-circuit is at lines 120–128. Replace it with a path-anchored check that covers both `.ironlint.yml` and `.ironlint/scripts/`:

```bash
# Short-circuit on edits to the policy surface: the on-disk hash won't match
# the trusted store while the user is mid-edit, so any `ironlint` invocation
# would fail the trust gate and surface a misleading "internal error". The
# policy surface is the config file (anywhere, matched by basename) AND the
# .ironlint/scripts/ directory (path-anchored to PROJECT_ROOT so a stray
# src/.ironlint/scripts/foo.sh is NOT matched).
BASENAME="${FILE##*/}"
if [[ "${BASENAME}" == ".ironlint.yml" ]]; then
  exit 0
fi
# Normalize to an absolute path for the prefix check (FILE may be relative).
case "${FILE}" in
  /*) abs_file="${FILE}" ;;
  *)  abs_file="${PROJECT_ROOT}/${FILE}" ;;
esac
if [[ "${abs_file}" == "${PROJECT_ROOT}/.ironlint/scripts/"* ]]; then
  exit 0
fi
```
(Place this where the old basename-only block was, after the `FILE` extraction at line 114 and before `run_ironlint` is defined.)

Also update the two bash-gate doc comments that mention `.ironlint/gates/` (lines 81, 219) to `.ironlint/scripts/`.

**codex hook** (`adapters/codex/hooks/hook.sh`): the basename short-circuit is at lines 261–266. Replace the `BASENAME` check with the same path-anchored logic (the codex hook already has `PROJECT_ROOT` from line 58):

```bash
  BASENAME="${ABSPATH##*/}"
  if [[ "${BASENAME}" == ".ironlint.yml" || "${BASENAME}" == ".bully.yml" ]]; then
    continue
  fi
  case "${ABSPATH}" in
    "${PROJECT_ROOT}/.ironlint/scripts/"*) continue ;;
  esac
```
And update the bash-gate doc comment at lines 72 and 219 from `.ironlint/gates/` to `.ironlint/scripts/`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p ironlint-cli --test hook_contract_claude_code
cargo test -p ironlint-cli --test hook_contract_codex
```
Expected: PASS — the `.ironlint/scripts/` write short-circuits; existing policy-file tests still pass (basename path unchanged for `.ironlint.yml`).

- [ ] **Step 5: Commit**

```bash
git add adapters/claude-code/hooks/hook.sh adapters/codex/hooks/hook.sh \
        crates/ironlint-cli/tests/hook_contract_claude_code.rs \
        crates/ironlint-cli/tests/hook_contract_codex.rs
git commit -m "feat(adapters): short-circuit edits under .ironlint/scripts/ (path-anchored)"
```

---

## Task 6: Update the pi and opencode TS adapters

**Files:**
- Modify: `adapters/pi/src/index.ts`
- Modify: `adapters/opencode/src/index.ts`
- Modify: `adapters/pi/test/index.test.ts`
- Modify: `adapters/opencode/tests/plugin.test.ts`

**Interfaces:**
- Consumes: `projectRoot` (already captured in both adapters: pi via `resolveRoot(pi)`, opencode via `worktree || directory`).
- Produces: `isPolicyFile(filePath, projectRoot)` — path-anchored. For pi it's an exported function (tested directly); for opencode it's module-private (tested via the plugin behavior).

- [ ] **Step 1: Write the failing tests.**

In `adapters/pi/test/index.test.ts`, add (adapting to the test runner's import style — `bun:test`'s `describe`/`it`/`expect`):

```ts
import { isPolicyFile } from "../src/index"

describe("isPolicyFile (gates→scripts)", () => {
  const root = "/proj"

  it("matches the config file by basename", () => {
    expect(isPolicyFile("/proj/.ironlint.yml", root)).toBe(true)
    expect(isPolicyFile(".ironlint.yml", root)).toBe(true)
  })

  it("matches files under .ironlint/scripts/ anchored to project root", () => {
    expect(isPolicyFile("/proj/.ironlint/scripts/lint.sh", root)).toBe(true)
    expect(isPolicyFile("/proj/.ironlint/scripts/sub/x.sh", root)).toBe(true)
  })

  it("does NOT match a .ironlint/scripts/ path outside the project root", () => {
    // src/.ironlint/scripts/foo.sh is NOT the policy surface — it must not
    // short-circuit, or a policy script could be hidden in a subdirectory.
    expect(isPolicyFile("/proj/src/.ironlint/scripts/foo.sh", root)).toBe(false)
  })

  it("does NOT match a legacy .ironlint/gates/ path (renamed away)", () => {
    expect(isPolicyFile("/proj/.ironlint/gates/lint.sh", root)).toBe(false)
  })
})
```

In `adapters/opencode/tests/plugin.test.ts`, mirror the same cases against the module's `isPolicyFile` (export it for testability if it isn't already — it's currently module-private at line 23; either export it or test via the `tool.execute.before` hook with a mocked `Bun.spawn` that panics-on-call, same pattern as the Rust hook tests). Prefer exporting `isPolicyFile` for direct unit testing:

```ts
import { isPolicyFile } from "../src/index"

describe("isPolicyFile (gates→scripts)", () => {
  const root = "/proj"
  it("matches config by basename and .ironlint/scripts/ anchored to root", () => {
    expect(isPolicyFile("/proj/.ironlint.yml", root)).toBe(true)
    expect(isPolicyFile("/proj/.ironlint/scripts/lint.sh", root)).toBe(true)
  })
  it("rejects scripts/ outside the project root", () => {
    expect(isPolicyFile("/proj/src/.ironlint/scripts/foo.sh", root)).toBe(false)
  })
  it("rejects the legacy .ironlint/gates/ path", () => {
    expect(isPolicyFile("/proj/.ironlint/gates/lint.sh", root)).toBe(false)
  })
})
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cd adapters/pi && bun test
cd ../opencode && bun test
```
Expected: FAIL — current `isPolicyFile` takes one arg (basename match) and doesn't know `.ironlint/scripts/`.

- [ ] **Step 3: Implement.**

**pi** (`adapters/pi/src/index.ts`): change `isPolicyFile` to take `projectRoot` and do a path-anchored check. Replace lines 109–117:

```ts
// R3: the policy surface. The config file (`.ironlint.yml` / `.bully.yml`,
// matched by basename so it works for relative and absolute paths) AND every
// file under `.ironlint/scripts/` (path-anchored to the project root so a
// stray `src/.ironlint/scripts/foo.sh` is NOT matched). Edits to these
// short-circuit the gate — checking a mid-edit policy file/script fails the
// trust gate (sha mismatch) and surfaces a confusing internal error.
const POLICY_FILES = new Set([".ironlint.yml", ".bully.yml"])

/** R3: basename match for the config file + path-anchored match for the
 *  `.ironlint/scripts/` directory. `projectRoot` anchors the scripts check. */
export function isPolicyFile(filePath: string, projectRoot: string): boolean {
  if (POLICY_FILES.has(basename(filePath))) return true
  const abs = isAbsolute(filePath) ? filePath : join(projectRoot, filePath)
  const scriptsDir = join(projectRoot, ".ironlint", "scripts") + sep
  return abs === scriptsDir.slice(0, -1) || abs.startsWith(scriptsDir)
}
```
Add the needed imports at the top: `isAbsolute, join, sep` from `node:path` (currently only `basename, join` are imported — add `isAbsolute` and `sep`). Update the call site in `ironlintExtension` (line 246): `if (isPolicyFile(filePath)) return` → `if (isPolicyFile(filePath, projectRoot)) return`. Update the bash-gate doc comment at line 219 (`.ironlint/gates/` → `.ironlint/scripts/`).

**opencode** (`adapters/opencode/src/index.ts`): export `isPolicyFile` and make it path-anchored. Replace lines 21–25:

```ts
const POLICY_FILES = new Set([".ironlint.yml", ".bully.yml"])

/** R3: config by basename + `.ironlint/scripts/` path-anchored to `projectRoot`. */
export function isPolicyFile(filePath: string, projectRoot: string): boolean {
  if (POLICY_FILES.has(basename(filePath))) return true
  const abs = isAbsolute(filePath) ? filePath : join(projectRoot, filePath)
  const scriptsDir = join(projectRoot, ".ironlint", "scripts") + sep
  return abs === scriptsDir.slice(0, -1) || abs.startsWith(scriptsDir)
}
```
Add `isAbsolute, sep` to the `node:path` import (line 3). Update the call site (line 130): `if (isPolicyFile(filePath)) return` → `if (isPolicyFile(filePath, projectRoot)) return`. Update the bash-gate doc comment at line 80 (`.ironlint/gates/` → `.ironlint/scripts/`).

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd adapters/pi && bun test
cd ../opencode && bun test
```
Expected: PASS — new policy tests green; existing tests that call `isPolicyFile` with the old one-arg signature must be updated (search for `isPolicyFile(` in both test files and add the `projectRoot` arg).

- [ ] **Step 5: Commit**

```bash
git add adapters/pi/src/index.ts adapters/pi/test/index.test.ts \
        adapters/opencode/src/index.ts adapters/opencode/tests/plugin.test.ts
git commit -m "feat(adapters-ts): path-anchored isPolicyFile for .ironlint/scripts/"
```

---

## Task 7: Update the cli.rs help text and the e2e trust test

**Files:**
- Modify: `crates/ironlint-cli/src/cli.rs` (Trust subcommand doc)
- Modify: `crates/ironlint-cli/tests/cli_e2e_trust.rs` (paths + summary assertions)
- Modify: `crates/ironlint-cli/tests/cli_e2e_doctor.rs` (path assertions)
- Modify: `crates/ironlint-core/tests/trust_extends.rs` (paths)

**Interfaces:**
- Consumes: Task 1 & 2 output (new summary shape; new directory).
- Produces: passing e2e tests; accurate CLI help.

- [ ] **Step 1: Write the failing tests.** Update the e2e tests to the new paths and summary shape.

In `crates/ironlint-cli/tests/cli_e2e_trust.rs`:

`trust_prints_blessed_summary` (≈ line 36) — rewrite to put the gate file under `.ironlint/scripts/` and assert `checks:` + `scripts:`:

```rust
#[test]
fn trust_prints_blessed_summary() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".ironlint.yml");
    fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \".ironlint/scripts/lint.sh\"\n",
    )
    .unwrap();
    let scripts = proj.path().join(".ironlint/scripts");
    fs::create_dir_all(&scripts).unwrap();
    fs::write(scripts.join("lint.sh"), "#!/bin/sh\nexit 0\n").unwrap();

    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(&cfg)
        .assert()
        .success()
        .stdout(
            predicates::str::contains("config sha256:")
                .and(predicates::str::contains("checks: 1"))
                .and(predicates::str::contains("scripts: 1"))
                .and(predicates::str::contains("lint.sh")),
        );
}
```

`trust_summary_omits_scripts_block_when_empty` (≈ line 71) → rename to `trust_summary_prints_zero_scripts_when_empty` and update the assertion (the scripts block is now always printed, showing `scripts: 0`):

```rust
#[test]
fn trust_summary_prints_zero_scripts_when_empty() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".ironlint.yml");
    fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
    )
    .unwrap();

    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(&cfg)
        .assert()
        .success()
        .stdout(
            predicates::str::contains("checks: 1")
                .and(predicates::str::contains("scripts: 0")),
        );
}
```

`editing_check_after_bless_blocks_check` (≈ line 226) — swap `.ironlint/gates/g.sh` → `.ironlint/scripts/g.sh` in both the config body (line 232) and the file setup (lines 235–237, 249):

```rust
    fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \".ironlint/scripts/g.sh\"\n",
    )
    .unwrap();
    let scripts = proj.path().join(".ironlint/scripts");
    fs::create_dir_all(&scripts).unwrap();
    fs::write(scripts.join("g.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    // ... (trust call unchanged) ...
    fs::write(scripts.join("g.sh"), "#!/bin/sh\nexit 2\n").unwrap(); // tamper
```

In `crates/ironlint-cli/tests/cli_e2e_doctor.rs`: find the assertion at line 303 (`"gates model doctor has 7 core checks"`) — update the string to `"scripts model doctor has 7 core checks"` (cosmetic; the test counts checks, doesn't touch paths). Search for any other `.ironlint/gates` literal in this file and swap to `.ironlint/scripts/`. (The `check_run_path_fails_missing_script` test here may duplicate the one in doctor.rs's inline tests — if so, update it too.)

In `crates/ironlint-core/tests/trust_extends.rs`: swap every `.ironlint/gates/` → `.ironlint/scripts/` (lines 6, 23, 28, 30, 69, 81 and the `base/.ironlint/gates/base.sh` setup). These are integration tests that a base config's script lives under the base's `.ironlint/scripts/`; the extends-injection invariant is unchanged, only the path moves.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p ironlint-cli --test cli_e2e_trust
cargo test -p ironlint-cli --test cli_e2e_doctor
cargo test -p ironlint-core --test trust_extends
```
Expected: FAIL — paths don't match the (post-Task-1) renamed directory; summary assertions expect `gates:`/`scripts:` in the old shape.

- [ ] **Step 3: Implement.** In `crates/ironlint-cli/src/cli.rs`, line 73:

```rust
    /// Bless this config + its `.ironlint/scripts/` scripts in the out-of-repo trust store.
    Trust {
```
(The test changes from Step 1 are the implementation for the test files themselves — they're both the test and the fix since they assert against the new shape produced by Tasks 1–2.)

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p ironlint-cli --test cli_e2e_trust
cargo test -p ironlint-cli --test cli_e2e_doctor
cargo test -p ironlint-core --test trust_extends
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-cli/src/cli.rs \
        crates/ironlint-cli/tests/cli_e2e_trust.rs \
        crates/ironlint-cli/tests/cli_e2e_doctor.rs \
        crates/ironlint-core/tests/trust_extends.rs
git commit -m "test+docs: update e2e + extends tests and CLI help for .ironlint/scripts/"
```

---

## Task 8: Full workspace verification + docs sweep

**Files:**
- Modify: all docs referencing `.ironlint/gates/` — `docs/architecture.md`, `docs/security/trust.md`, `docs/writing-checks/recipes.md`, `docs/writing-checks/README.md`, `docs/getting-started.md`, `docs/adapters/README.md`, `docs/configuring/targeting-files.md`, `docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`, `docs/superpowers/plans/2026-07-06-bash-gate-self-trust-prevention.md`, `CLAUDE.md`
- Modify: `crates/ironlint-cli/src/commands/check.rs` — one doc comment at line 96 (`This hashes the config + .ironlint/gates/ now` → `.ironlint/scripts/`).

**Interfaces:** none (verification + prose).

- [ ] **Step 1: Run the full Rust suite + clippy + fmt + coverage gate.**

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt -- --check
bash scripts/ci-coverage.sh
```
Expected: all PASS. Coverage gate: every touched `crates/*/src/*.rs` file at ≥80% region coverage. If `ci-coverage.sh` leaves `target/llvm-cov-target` / `target/llvm-cov` scratch (known), clean it: `rm -rf target/llvm-cov-target target/llvm-cov` (per memory `ci-coverage-cleanup`).

- [ ] **Step 2: Run the TS adapter suites.**

```bash
cd adapters/pi && bun test && cd ../..
cd adapters/opencode && bun test && cd ../..
```
Expected: PASS.

- [ ] **Step 3: Verify no `.ironlint/gates` literal remains in live source/tests/docs.**

```bash
grep -rn --include='*.rs' --include='*.sh' --include='*.ts' --include='*.md' --include='*.yml' \
  '.ironlint/gates' . | grep -v './target/' | grep -v './node_modules/' | grep -v './.claude/worktrees/'
```
Expected: **empty output**. (The `.claude/worktrees/init-harness-toggle/` worktree is stale and out of scope — it's a separate branch's checkout; do not edit it.) Any remaining hit in `docs/` or `specs/` is a docs-debt finding to fix in Step 4.

- [ ] **Step 4: Docs sweep.** For each docs file from Step 3's pre-filter list, replace `.ironlint/gates/` → `.ironlint/scripts/` and, where prose says "gates directory" or "gate scripts" in the policy-directory sense, reword to "scripts directory" / "policy scripts". Be surgical: do **not** touch the word "gates" where it means the bash-gate feature or the check verdict ("gate" as in `gate::run_gate`) — only the `.ironlint/gates/` directory references. Update `CLAUDE.md`'s `gate-bash` paragraph and the trust-surface description to say `.ironlint/scripts/`. Update the bash-gate self-trust spec (`docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`) and its plan to reference `.ironlint/scripts/`. Also fix the one source-comment hit outside docs: `crates/ironlint-cli/src/commands/check.rs:96` (`This hashes the config + .ironlint/gates/ now` → `.ironlint/scripts/`).

- [ ] **Step 5: Final smoke test.** Build release and exercise the renamed flow end-to-end:

```bash
cargo build --release
# in a scratch dir:
mkdir -p /tmp/il-smoke/.ironlint/scripts
cat > /tmp/il-smoke/.ironlint.yml <<'YML'
checks:
  no-todo:
    files: "*.md"
    run: ".ironlint/scripts/no-todo.sh"
YML
cat > /tmp/il-smoke/.ironlint/scripts/no-todo.sh <<'SH'
#!/bin/sh
grep -q TODO "$IRONLINT_FILE" && { echo "no TODOs allowed"; exit 2; } || exit 0
SH
chmod +x /tmp/il-smoke/.ironlint/scripts/no-todo.sh
cd /tmp/il-smoke
XDG_CONFIG_HOME=/tmp/il-smoke/xdg ../../Users/chrisarter/Documents/projects/ironlint/target/release/ironlint trust
echo "TODO: fix me" > a.md
/Users/chrisarter/Documents/projects/ironlint/target/release/ironlint check --file a.md  # expect exit 2
echo "clean" > b.md
/Users/chrisarter/Documents/projects/ironlint/target/release/ironlint check --file b.md  # expect exit 0
# bash-gate blocks a write to the scripts dir:
echo 'x' | /Users/chrisarter/Documents/projects/ironlint/target/release/ironlint gate-bash  # then test: `echo y > .ironlint/scripts/evil.sh | gate-bash` should be blocked by running the hook path
cd /Users/chrisarter/Documents/projects/ironlint
rm -rf /tmp/il-smoke   # cleanup the scratch artifact per the cleanup rule
```
Expected: `trust` prints `checks: 1` and `scripts: 1` with `no-todo.sh`; the TODO file blocks (exit 2); the clean file passes (exit 0); a Bash write to `.ironlint/scripts/evil.sh` is blocked by `gate-bash` (exit 2). Drop the scratch dir after.

- [ ] **Step 6: Commit**

```bash
git add docs/ CLAUDE.md
git commit -m "docs: rename .ironlint/gates/ to .ironlint/scripts/ across docs and specs"
```

- [ ] **Step 7: Request code review.** Per the repo's `CLAUDE.md` rule ("After completing a coding task, request code review from a separate agent"), dispatch a review subagent over the full diff (`git diff main`). Use the `superpowers:requesting-code-review` skill.

---

## Self-Review Notes

**Spec coverage check** (against `docs/superpowers/specs/2026-07-07-gates-to-scripts-rename-design.md`):
- "User-visible changes" — `.ironlint/gates/` retired → Tasks 1, 4, 5, 6, 7. `ironlint trust` output `checks:`/`scripts:` → Task 2. `doctor` checks `.ironlint/scripts/*` → Task 3. `gate-bash` blocks writes to `.ironlint/scripts/` → Task 4. `init` scaffolds baseline inline checks (no scripts dir) → already true (init scaffolds inline checks; verified `init.rs` creates no gates dir). ✓
- "Architecture / Trust hash" — walk `.ironlint/scripts/`, drop referenced-scripts fold → Task 1. ✓
- "Bash-gate policy surface" → Task 4. ✓
- "Adapter hooks" path-based short-circuit → Task 5 (shell) + Task 6 (TS). ✓
- "BlessedSummary" fields → Task 1. ✓
- "Security model" — AI can't run `ironlint trust` (bash-gate, Task 4), can't write policy via Bash (Task 4), exit 4 until re-trust (existing, unchanged) → covered. ✓
- "Files touched" — all listed files have a task; `gate_bash.rs` correctly identified as needing no change. ✓
- "Testing" — every test in the spec's Testing section has a step: trust.rs tests (T1), doctor.rs tests (T3), gate_bash.rs tests (T4 — note: the actual tests are in `ironlint-bash-gate/src/lib.rs`, the spec's "gate_bash.rs tests" means the bash-gate tests), adapter hook allows `.ironlint/scripts/foo.sh` while untrusted (T5), trust output shows `checks: N`/`scripts: N` (T2/T7), bash-gate blocks `cp x .ironlint/scripts/foo.sh` (T4, line 765). ✓

**Placeholder scan:** no TBD/TODO/"handle edge cases". Every code step shows the actual code. The one soft spot is Task 5 Step 1's `Fx` fixture — it defers to the existing `hook_contract_claude_code.rs` fixture API; the implementer reads that file first and mirrors an existing test, which is the established pattern (not a placeholder — it's a deliberate "match the surrounding code" instruction with the invariant spelled out).

**Type consistency:** `BlessedSummary` is `{ config_path, config_hash, checks: usize, scripts: Vec<String> }` in Task 1 (producer), Task 2 (consumer — `render_summary`), and Task 7 (e2e assertions). `isPolicyFile(filePath, projectRoot)` signature is consistent across Task 6's pi and opencode. `is_policy_path` stays `&str -> bool` in Task 4. `closure_script_dirs` / `collect_gate_files` names are consistent within Task 1. ✓
