# Arch feature test-coverage gaps

Source: test-quality audit of the architecture-enforcement feature run 2026-07-10
(merge-review branch `f12f152..9fac6f7`). Methods: three parallel review agents
classified every arch test by tier and verified each of the 10 merge-review bug
fixes has a regression test that fails on revert; `cargo-mutants` run as
empirical ground-truth (50/53 mutants caught → 3 survivors); full arch test
suite green (37 passed, 8 suites).

Each item below is self-contained — an agent can be dispatched against one
without reading the others. Items are ordered by impact (critical coverage
gaps first, then polish).

Base reference for the 10 original bugs:
`docs/audits/2026-07-10-architecture-enforcement-merge-review.md`.

---

## 1. Two surviving mutants in the Bug-4 tsconfig alias resolver [CRITICAL]

`cargo-mutants` proved these two mutations go uncaught. Both sit in the
alias-resolution code fixed for Bug 4 (tsconfig.paths longest-match + all
targets). Alias resolution is exactly where a silent wrong-target is
dangerous (forbidden import resolves to the wrong layer → false Pass).

### 1a. Longest-match sort key arithmetic not pinned

- **Mutant (survived):** `crates/ironlint-core/src/arch/languages/typescript.rs:135`
  - `replace - with /` in `resolve_alias`
  - `let consumed = spec.len() - suffix.len();` → `spec.len() / suffix.len();`
- **Why it survives:** `consumed` is the sort key that picks the
  longest-matching alias pattern (Bug 4). The existing test
  `alias_longest_match_wins` (`typescript.rs:365`) uses two aliases whose
  `consumed` values happen to keep the same relative order under `/` as
  under `-`. So flipping `-` to `/` does not reorder them and the test
  still passes.
- **Risk:** A future alias pair whose `consumed` values reorder wrongly
  under `/` would silently pick the wrong (broader) target — the original
  Bug 4 failure mode — and no test would catch it.
- **Fix — write a failing test first** (`crates/ironlint-core/src/arch/languages/typescript.rs`,
  inline `mod resolver_tests`):
  - Choose two alias patterns + a spec such that
    `spec.len() - suffix.len()` (correct) orders them one way, but
    `spec.len() / suffix.len()` (mutated) orders them the *other* way, and
    the wrong order resolves to a different file on disk.
  - Concrete approach: pick alias A and alias B where
    `consumed_A > consumed_B` under `-` but
    `(spec.len() / suffix_A.len()) < (spec.len() / suffix_B.len())` under
    `/`. Place a forbidden-import target file at the correct-resolution
    path and a *different* file at the wrong-resolution path. Assert the
    resolver returns the correct path.
  - Verify: with the mutant applied (`-`→`/`), the new test fails; with
    the fix in place, it passes. (Run `cargo mutants --file
    crates/ironlint-core/src/arch/languages/typescript.rs` after to
    confirm the mutant is now caught.)

### 1b. Non-wildcard alias (`alias == spec`) branch untested

- **Mutant (survived):** `crates/ironlint-core/src/arch/languages/typescript.rs:161`
  - `replace == with !=` in `match_alias`
  - `else if alias == spec` → `else if alias != spec`
- **Why it survives:** `match_alias`'s `else if alias == spec` arm handles
  a **bare alias** — one without `/*`, e.g. `"@/utils": ["src/utils.ts"]`.
  Every existing alias test uses wildcard patterns (`@/*`, `@/data/*`).
  Flipped to `!=`, a bare alias would match *any* spec, but no test
  exercises a bare alias so nothing fails.
- **Risk:** A config with a non-wildcard alias would mis-resolve: the
  alias matches every spec, so `@/anything` could resolve to the bare
  alias's target. Forbidden import resolves to the wrong layer → false
  Pass.
- **Fix — write failing tests** (same inline `mod resolver_tests`):
  - `tsconfig` with a non-wildcard alias, e.g.
    `"@/utils": ["src/utils.ts"]` (no `/*`). Assert `resolve("@/utils",
    ...)` returns `Some(src/utils.ts)`.
  - Assert `resolve("@/something-else", ...)` returns `None` (the bare
    alias must match only its exact spec, not everything). This is the
    assertion that kills the `!=` mutant.
  - Verify both fail with `==`→`!=` applied, pass with the fix.

---

## 2. Bug-8 SIGKILL temp-leak: sweep-on-load integration call is untested [CRITICAL]

The Bug-8 fix has two parts: (a) the sweep *function* recognizes
`ironlint-arch-*` files, and (b) the sweep is *called* on engine load at
`crates/ironlint-core/src/runner.rs:697`
(`sweep_stale_tmpfiles(&std::env::temp_dir(), ...)`).

- **What IS tested:** the `sweep_stale_tmpfiles` function — 6 inline unit
  tests in `runner.rs:1884-1999` (removes old `ironlint-arch-*`, keeps
  fresh, ignores dirs, tolerates missing root, nested-dir sweep). Removing
  `ARCH_LAYERS_PREFIX` from the prefix list fails
  `sweep_stale_tmpfiles_removes_old_arch_layers_files` (runner.rs:1943).
- **What is NOT tested:** the call site at `runner.rs:697`. If that line
  were deleted entirely, **no test would fail.** No test places a stale
  `ironlint-arch-*` file in the system temp dir, loads an `IronLintEngine`,
  and asserts the file was reclaimed. The actual SIGKILL-then-reload
  scenario the fix exists for is unexercised.
- **Feasibility note:** the audit doc hedged this ("*if* the harness can
  do it"), but the `nix` crate is already a dev-dependency (used by
  `gate.rs:659`'s process-group kill test), so the harness CAN do a kill.
  A full spawn→SIGKILL→reload test is possible but heavy; the lighter,
  equally-valid fix is to pin the integration call site directly.
- **Fix — write a failing test** (`crates/ironlint-core/src/runner.rs`
  inline `#[cfg(test)]`, or `tests/`):
  - In a controlled temp dir, create a stale `ironlint-arch-<pid>-<n>.yml`
    file and backdate its mtime ~2h (the existing sweep tests show the
    age-backdating pattern).
  - Point `TMPDIR` at that temp dir (the sweep targets
    `std::env::temp_dir()`, which honors `TMPDIR`). Use
    `std::env::set_var("TMPDIR", ...)` scoped to the test (restore in a
    guard / `Drop` — be mindful of test parallelism; consider
    `#[serial]` or a dedicated test binary).
  - Load an `IronLintEngine` (the load path is what calls the sweep at
    `runner.rs:697`).
  - Assert the stale `ironlint-arch-*` file no longer exists.
  - Verify: delete/comment the `sweep_stale_tmpfiles(&std::env::temp_dir(),
    ...)` call at `runner.rs:697` → test fails. Restore it → passes.
  - Alternative if `TMPDIR`-mutation is too fragile in the parallel test
    runner: refactor the call site to take the temp dir as a parameter
    (defaulting to `std::env::temp_dir()`) so the test can inject a path
    directly. This is a small, safe refactor that makes the integration
    testable without env-var gymnastics.

---

## 3. Bug-10 symlink-path regression is macOS-only [HIGH]

`why_finds_violations_via_symlinked_path` (`crates/ironlint-core/tests/arch_engine.rs:192`)
is `#[cfg(target_os = "macos")]` because it relies on the OS-provided
`/tmp` → `/private/tmp` symlink. The canonicalization fix it guards
(`ArchEngine::why` canonicalizing the requested path via
`canonicalize_through_parent`) has **zero coverage on Linux CI.** If a
refactor drops the canonicalization, Linux CI stays green while macOS
fails — and a Linux-only contributor's `cargo test` wouldn't catch it
either.

- **Fix — write a portable test** (same file, no `cfg` gate):
  - Use `std::os::unix::fs::symlink` to create a real symlink under a
    tempdir (works on both Linux and macOS). Build the arch repo under
    the canonical target dir, then query `ArchEngine::why` with the
    symlinked (non-canonical) path alias.
  - Assert the violation is found (count + importer + `rule_from`).
  - Delete the macOS-only `#[cfg(target_os = "macos")]` test once the
    portable one covers the same code path — or keep both if the macOS
    `/tmp`→`/private/tmp` case is considered worth pinning specifically.
  - Verify: remove the `canonicalize_through_parent(path)` call in
    `ArchEngine::why` (`crates/ironlint-core/src/arch/engine.rs`) → both
    the macOS test and the new portable test fail.

---

## 4. `walk_files` directory-skip mutant survives [MEDIUM]

- **Mutant (survived):** `crates/ironlint-core/src/arch/graph.rs:340`
  - `replace || with &&` in `walk_files`
  - `if name == ".git" || name == "node_modules"` → `&&`
- **Why it survives:** with `&&`, a dir is skipped only if named BOTH
  `.git` AND `node_modules` (impossible), so neither is ever skipped.
  Every arch test fixture is a clean tempdir with no `.git` or
  `node_modules` directory, so nothing exercises the skip.
- **Risk:** a repo being linted that contains `node_modules/` (with `.ts`
  files inside) would have its entire dependency tree walked and
  classified — slow, noisy, and can produce false violations from
  third-party code.
- **Fix — write a failing test** (`crates/ironlint-core/tests/arch_graph_build.rs`
  or inline in `graph.rs`):
  - Build a repo fixture containing a `node_modules/` dir and a `.git/`
    dir, each with a `.ts` file holding a forbidden import (e.g.
    `node_modules/pkg/index.ts` importing `../data/db`).
  - Call `DepGraph::build` (or `ArchEngine::check_whole`).
  - Assert the graph contains NO nodes under `node_modules/` or `.git/`
    (the dirs were skipped, not walked). Equivalently assert
    `check_whole` returns `Pass`/empty despite the forbidden import
    sitting inside the skipped dir.
  - Verify: apply `||`→`&&` → the forbidden import inside
    `node_modules/` is walked and produces a violation → test fails.

---

## 5. Two misleading overlay tests [LOW — polish]

These don't represent missing coverage, but their names claim properties
they don't actually verify. They erode trust in the suite.

### 5a. `manifest_proposed_overrides_disk_version`

- **File:** `crates/ironlint-core/tests/arch_overlay.rs:101`
- **Problem:** the name claims to verify the manifest *overrides* the disk
  version, but `db.ts` IS on disk, so the forbidden import resolves to
  the on-disk file regardless of whether the overlay is applied. `Block`
  occurs with or without `merge_proposed` — the test would pass even if
  the overlay did nothing.
- **Note:** the graph-level version (`graph.rs:541
  merge_proposed_overrides_disk_version`) correctly verifies the override
  by checking the virtual node's *edges differ* from the disk node's. So
  the property IS covered — just not at the engine level this test
  claims to.
- **Fix:** either rewrite to put `db.ts` off-disk so the manifest is what
  actually produces the `Block` (rather than the on-disk file), or delete
  this engine-level test in favor of the graph.rs:541 version that
  actually verifies the property.

### 5b. `empty_manifest_is_noop`

- **File:** `crates/ironlint-core/tests/arch_overlay.rs:203`
- **Problem:** `db.ts` IS on disk, so `Block` comes from the on-disk
  resolution — not from anything the (empty) manifest does or doesn't do.
  A true noop test would have `db.ts` off-disk + an empty manifest →
  `Pass` (manifest adds no virtual nodes → import unresolved → no
  violation).
- **Fix:** rewrite so `db.ts` is NOT on disk; assert `Pass` with an empty
  manifest (and the existing `manifest_with_blank_lines_is_tolerant`
  style). This actually pins the noop property.

---

## 6. (Optional) Engine/CLI-level integration tests for Bug 3 and Bug 4 [LOW]

Both bugs are covered well at the unit level (`for_path` routing for Bug 3;
`TypescriptResolver` for Bug 4) and both *would* fail on revert at that
level. There is no `ArchEngine`/CLI test exercising JSX-before-import (Bug
3) or tsconfig aliases (Bug 4) end-to-end through `DepGraph::build`. The
unit tests are sufficient for the fix surface; an engine-level test
would be defense-in-depth.

- **Bug 3 engine test:** a `.tsx` repo with JSX before a forbidden import,
  assert `ArchEngine::check_whole` → `Block`. (The unit test at
  `mod.rs:36` covers the routing; this would cover the full build path.)
- **Bug 4 engine test:** a repo with a `tsconfig.json` containing both a
  broad `@/*` and specific `@/data/*` alias, a forbidden import using
  `@/data/db`, assert the graph resolves to the correct target layer and
  `check_whole` → `Block` (or `Pass` if the target is in an allowed
  layer).

Lower priority than 1–4 — only do these if hardening the integration
layer is desired. Note item 1's fixes already strengthen Bug-4 unit
coverage.

---

## Verification protocol for each fix

Per repo rules (`CLAUDE.md`): bug fixes start with a failing test. For
each item above:

1. Write the failing test.
2. Confirm it fails (run the specific test).
3. Confirm it fails *for the right reason* — apply the named mutant /
   delete the named call / flip the named operator and verify the test
   catches it (this is what makes it a real regression test, not a
   tautology).
4. Apply the fix (or, for items 1–4, the fix is already in the production
   code — the test is the deliverable; for item 5, the rewrite is the
   deliverable).
5. Confirm the test passes with the fix.
6. Run `cargo mutants --file <path>` (items 1, 4) to confirm the
   previously-surviving mutant is now caught.
7. Run `cargo test --workspace --locked` to confirm no regressions.
8. Clean up any scratch artifacts (`pr.diff`, mutant output dirs).
