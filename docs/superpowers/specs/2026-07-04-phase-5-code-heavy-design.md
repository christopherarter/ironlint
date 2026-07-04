# Phase 5 — Code-heavy remaining tasks (design)

**Date:** 2026-07-04
**Status:** Design (pending implementation plan)
**Scope:** Phase 5 readiness-review items that are code-heavy: 5.25, 5.26, 5.27, 5.28, 5.29, 5.31.

**Out of scope (deferred):**
- All adapter tasks (5.20–5.24) — no extra adapter maintenance burden yet; docs for building one's own adapter will land in the docs wrap-up.
- Group A docs (5.1–5.8), backlog adapters (5.32–5.33) — wrap-up phase.
- 5.14 (rotate the leaked `.env` key) — ops/action item, tracked as a separate follow-up.

---

## 1. Architecture & execution model

**Approach B — max parallelism on independent files; sequence the one interdependent task; run CI work last.**

```
Wave 1 (parallel, 4 sessions — independent files):
  5.25  fuzz/                    (new fuzz crate, no existing-file conflicts)
  5.26  scope.rs proptest        (new test module in scope.rs)
  5.28  mutants.out refresh      (investigative — produces evidence, no merge)
  5.31  trust.rs UX              (trust.rs + trust.rs core summary fn)

Wave 2 (single-threaded, one SDD task with 3 commits):
  5.29  perf — prefilter + telemetry rotation + incremental watch
        (runner.rs + telemetry.rs + watch.rs — share core data structures)

Wave 3 (last — tests the stabilized binary):
  5.27  release.yml smoke job
```

**Why this shape:**
- Wave 1 tasks touch **4 disjoint files** (`fuzz/`, `scope.rs`, `mutants.out/`, `trust.rs`+`trust core`) — zero merge-conflict risk.
- 5.29's three sub-items share `runner.rs`/`telemetry.rs`/`watch.rs` and must stay in one head to avoid conflicts. One task, three commits, not three parallel tasks.
- 5.27 goes last: the smoke job runs the shipped binary, so it should test the final shape (post-5.29 perf changes), not a moving target.

Each task gets a brief (`.superpowers/sdd/task-N-brief.md`), a RED→GREEN TDD cycle, and a separate review pass before merge — matching the prior phase-5 SDD artifacts.

---

## 2. Components — per-task designs

### 5.25 — Fuzz the two parse surfaces  `[testing]`
- New `fuzz/` directory at workspace root (cargo-fuzz convention). Two targets:
  - `fuzz_targets/config_parser_fuzz.rs` → calls `ironlint_core::config::parser::parse_str(data.as_bytes())`, asserts no panic/abort (errors are fine — it's a `Result`).
  - `fuzz_targets/diff_parser_fuzz.rs` → calls `ironlint_core::diff::parser::parse_unified(data.as_bytes())`, same no-panic invariant.
- `cargo-fuzz` is a dev-tool, not a runtime dep (`cargo +nightly` only). `fuzz/Cargo.toml` is a separate crate with a `path` dep on `ironlint-core`.
- **TDD angle:** fuzz targets are *invariant* tests (no panic), not assertion tests. RED = the target panics on a known corpus seed (reproduce a known parser edge, e.g., the git-quoted path from 5.10); GREEN = target handles it without panic.
- **CI:** a short nightly fuzz smoke (30s per target), gated behind a schedule, not per-PR (would burn minutes). Local: `cargo +nightly fuzz run config_parser -- -max_total_time=30`.
- **Files:** new `fuzz/Cargo.toml`, `fuzz/fuzz_targets/{config,diff}_parser_fuzz.rs`, `fuzz/corpus/{config,diff}/` seed dirs.

### 5.26 — Property-test scope matching  `[testing]`
- Add `proptest` to `ironlint-core` dev-deps. New `#[cfg(test)] mod proptest` in `scope.rs`.
- Properties (the invariants that must hold):
  1. **Reflexivity:** a glob literal matches its own path (`*.rs` matches `foo.rs`).
  2. **Bare-pattern depth invariance:** `*.py` matches at any depth (`a/b/c/x.py`), per the documented scope-divergence.
  3. **Negation correctness:** a negated glob (`!vendor/`) excludes what its positive form includes.
  4. **Determinism:** `ScopeMatcher::new(&[globs]).matches(p)` is stable across calls (proptest shrinks to minimal failing case).
- Seed generators: valid glob strings from a small alphabet (`*`, `**/`, `?`, `*.ext`, `/`, `[a-z]`) and random relative paths.
- **Files:** `crates/ironlint-core/Cargo.toml` (add proptest dev-dep), `crates/ironlint-core/src/config/scope.rs` (new test module).

### 5.28 — Refresh mutation-testing evidence  `[testing]`
- **Investigative, not a merged code change.** Delete stale `mutants.out/` and `mutants.out.old/` (pre-0.4-redesign, May-12). Re-run `cargo mutants` against phase-5-touched files: `git diff main.. > pr.diff && cargo mutants --in-diff pr.diff`.
- Output: a **report** (`.superpowers/sdd/mutants-report.md`) listing surviving mutants in phase-5 code. Each survivor in code we touched is a coverage gap → feeds back into 5.25/5.26/5.29 as test additions before merge.
- No merge to `main` from this task itself; it produces evidence that may spawn fix commits in the other tasks.
- **Files:** `mutants.out/` (delete), new report file.

### 5.31 — `ironlint trust` shows what it blesses  `[cli]`
- Today: `trust::run` calls `bless(&config)` and prints `trusted: <path>`. No summary of *what* was blessed.
- Add a core summary function — `trust::blessed_summary(config: &Path) -> BlessedSummary` returning `{ config_path, config_hash, gate_count, gate_paths }` — so the CLI layer stays thin (per the project's CLI-as-adapter convention).
- CLI prints a multi-line summary:
  ```
  trusted: /path/to/.ironlint.yml
    config sha256: <16-char prefix>
    gates: 3
      - gates/no-todo.sh
      - gates/no-debug.py
      - gates/format-rs.sh
  ```
- The hash + file list already exist in `trust.rs` core (the blessing computes them) — expose them without recomputing.
- **TDD:** RED = `trust` test asserting the output contains the gate count + first gate path; GREEN wires the summary through.
- **Files:** `crates/ironlint-core/src/trust.rs` (new summary fn), `crates/ironlint-cli/src/commands/trust.rs` (print summary).

### 5.29 — Perf: prefilter + telemetry rotation + incremental watch  `[perf]`
Three sub-items, one task, three commits (shared files):

1. **Prefilter (P3)** — `runner.rs`: after config resolution + the trust gate pass, cheaply check whether *any* check's `files` globs could match the target path. If none can, return `Pass` fast without entering the per-check spawn loop. **Ordering:** config-resolution → trust gate (`exit 4` if untrusted) → prefilter → (spawn loop, or fast-pass). **Security invariant:** a non-matching file legitimately runs nothing, so fast-pass is correct — but the trust gate must still run first (an untrusted repo with a non-matching file is still exit 4, not silent pass). The prefilter only skips the spawn loop, never the trust check.
   - Invariant test: prefilter returns `Pass` (no spawn) when no glob matches; trust gate still fires on untrusted config.
2. **Telemetry rotation (P4)** — `telemetry.rs`: size-based rotation of `.ironlint/log.jsonl` → `.ironlint/log.jsonl.1` (keep N=1 old copy; rotate when size > 10 MiB). Use `fs2` (already a workspace dep) for the size check. Atomic-rename, then fresh append.
   - Test: append entries past the threshold → `.1` appears, current file is post-rotation size.
3. **Incremental watch (P4)** — `watch.rs`: track a byte offset in `ViewState`; each tick, `seek(SeekFrom::Start(offset))` + read new tail + parse only the appended lines. Reset offset to 0 when the file shrinks (rotation or truncation — handle both).
   - Test: append N lines after offset O → watch yields exactly the N new lines, not all O+N.

All three touch core data flow → one SDD task with three commits, no parallelization within 5.29.

### 5.27 — Release smoke verification  `[testing]`
- Add a `smoke` job to `.github/workflows/release.yml` (dist-generated). Dist doesn't manage "post-release smoke" jobs natively — add a hand-edited job **outside dist's managed block** with a clear comment so `dist generate` won't fight it.
- Triggers on `published: release` (post-tag). On each OS target, downloads the just-built artifact, runs the shell installer into a temp prefix, executes `ironlint --version` + a one-check run against a tiny fixture repo.
- **TDD angle:** the smoke job is itself the test; locally verified by running the installer script manually against a release-candidate tag.
- **Files:** `.github/workflows/release.yml` (new smoke job).

---

## 3. Data flow & dependencies

```
Wave 1 (parallel):
  5.25 fuzz/         ──┐
  5.26 scope proptest ─┼──► (merge to main, individually reviewed)
  5.28 mutants report ─┼──► feeds findings back → may add tests to 5.25/5.26/5.29
  5.31 trust UX      ──┘    before merge (mutant survivors in touched code = blockers)

Wave 2 (sequential, after Wave 1 merges):
  5.29 perf (3 commits) ──► prefilter → telemetry rotation → incremental watch
                           (each commit GREEN before next; shared files → no parallel)

Wave 3 (after 5.29 merges — binary shape is final):
  5.27 release smoke ──► tests the shipped binary in CI
```

**Cross-task dependencies:**
- 5.28 (mutants) can feed findings into any Wave 1 task still in review — it's investigative, runs alongside. If a mutant survives in `scope.rs` and 5.26 is still open, the fix lands in 5.26 before merge.
- 5.29 depends on Wave 1 merging first (clean working tree, no `runner.rs`/`telemetry.rs` conflicts with anything in flight).
- 5.27 depends on 5.29 (binary perf shape final) — testing a moving target wastes CI.

---

## 4. Error handling

- **5.25 fuzz:** targets must never panic — they return `Result` and the harness asserts no-panic. A discovered panic is a bug in the parser, fixed in `ironlint-core` (feeds back as a real fix commit, not part of the fuzz task itself).
- **5.26 proptest:** shrinking finds minimal failing cases; a failing property is a real scope-matching bug, fixed in `scope.rs`.
- **5.29 prefilter:** security invariant — *trust gate still fires on untrusted config even when no glob matches*. Fast-pass only skips the spawn loop, never the trust check. Exit 4 still emitted for untrusted.
- **5.29 telemetry rotation:** atomic-rename; a rotation failure mid-write must not lose the in-flight entry (write first, rotate after).
- **5.29 incremental watch:** if the file shrunk since last offset (rotation or truncation), reset offset to 0 and re-read fully — never yield stale or negative-offset reads.

---

## 5. Testing

Every code task follows the project's TDD rule (RED → GREEN → commit):

- **5.25:** corpus seed reproducing a known parser edge (e.g., the git-quoted path from 5.10) → fuzz target handles it without panic.
- **5.26:** proptest properties (reflexivity, bare-pattern depth invariance, negation correctness, determinism).
- **5.28:** the report is the deliverable; mutant survivors in touched code become test additions in the relevant task.
- **5.31:** `trust` output contains the config hash, gate count, and at least one gate path.
- **5.29 prefilter:** no-spawn-on-no-match + trust-gate-still-fires-on-untrusted.
- **5.29 rotation:** `.1` file appears past threshold; current file is post-rotation size.
- **5.29 incremental watch:** appended lines yield exactly the new tail.
- **5.27:** smoke job runs `ironlint --version` + one-check run end-to-end on each OS.

All code tasks meet the ≥80% region coverage gate (per-file, CI-enforced via `scripts/ci-coverage.sh`) and ≤15 cognitive complexity (clippy). Fuzz/proptest/mutants are dev-only — no runtime dep weight added to the shipped binary.

---

## 6. Verified code anchors (do not re-derive)

- `telemetry.rs:65` — `append()` uses `OpenOptions::new().append(true).create(true)`; no rotation, no size cap. Lines 117/141 — `read_all`/`read_all_quiet` re-read the whole file.
- `watch.rs:341` — calls `read_all_quiet` every 250ms tick (full re-read, no offset tracking).
- `release.yml` — single `jobs:` block at line 74, dist-generated; `pr-run-mode = "plan"` in `Cargo.toml:28`; no smoke job.
- `diff/parser.rs:105` — `parse_unified(input: &str)`; `config/parser.rs:10` — `parse_str(input: &str)`. Both are clean fuzz targets.
- `scope.rs` — `ScopeMatcher::new(globs) -> Result<Self>` + `matches(path) -> bool`. Ideal proptest surface.
- `trust.rs` (CLI) — 15-line `run()` that calls `bless()` and prints one line; 5.31 adds a summary of what was blessed.
- Workspace has `tempfile`, `assert_cmd`, `insta`, `fs2` — no `proptest`, no `cargo-fuzz` yet.

---

## 7. Sequencing & dispatch summary

| Wave | Task | Files | Sessions |
|------|------|-------|----------|
| 1 | 5.25 fuzz | new `fuzz/` | 1 |
| 1 | 5.26 proptest | `scope.rs`, core `Cargo.toml` | 1 |
| 1 | 5.28 mutants | `mutants.out/`, report | 1 |
| 1 | 5.31 trust UX | `trust.rs` (core + cli) | 1 |
| 2 | 5.29 perf (3 commits) | `runner.rs`, `telemetry.rs`, `watch.rs` | 1 (single-threaded) |
| 3 | 5.27 release smoke | `release.yml` | 1 |

Wave 1 = 4 concurrent sessions (matches plan cap). Wave 2 = 1 session (sequential commits). Wave 3 = 1 session.
