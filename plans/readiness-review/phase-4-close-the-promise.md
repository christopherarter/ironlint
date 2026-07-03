# Phase 4 — Close the promise

**Goal:** make the "one config, enforced everywhere" and "local CI" claims
literally true — reach the harnesses and channels where the users actually are,
and close the write→commit→CI loop.

These are **larger tasks** than Phases 1–3. Each is a small project. They carry
more design latitude, so each task states the decisions made for you and flags
where to confirm details against a live tool or upstream docs (which may have
changed since this was written — verify before building).

**Read [`README.md`](README.md) first.** Standard Verification still applies; new
adapters get their own tests mirroring the pi/opencode suites.

### Dependency graph for this phase

```
4.1  Cursor adapter            (independent)
4.2  GitHub Action (CI)        (independent)
4.3  pre-commit git hook       (independent)  ─┐
4.4  staged-content reading    depends on 4.3 ─┘
4.5  distribution (brew/npm)   (independent)
4.6  parallel dispatch + cache depends on 2.1 (process groups)
4.7  extends from git URLs     depends on 3.3 (trust of referenced files)
```

---

## Task 4.1 — Build the Cursor adapter  [Finding M1]

- **Severity:** Blocker (for "multi-harness" credibility) · **Effort:** L
  · **Depends on:** none
- **Files:** new `adapters/cursor/`, `crates/ironlint-core/src/adapter/
  registry.rs` (register the harness), init/doctor wiring, `docs/adapters/
  cursor.md`, tests mirroring an existing adapter suite.

### Why
Cursor is the #2 harness by mind-share and #1 by revenue (~$2B ARR, 1M+ paying
seats). It ships hooks (`afterFileEdit`, `beforeShellExecution`, etc.) with
JSON-on-stdin and exit-code/permission block semantics — the same shape the
claude-code adapter already handles. Its absence means the "multi-harness" pitch
excludes the largest commercial population.

### Before you build — verify the current contract
The hook API may have changed since this plan was written. Confirm against the
live docs (https://cursor.com/docs/hooks) and a real `~/.cursor` install:
- The exact hook event for a **pre-write** gate (prefer a before-write/before-
  apply event so you can block before disk; if only `afterFileEdit` exists, the
  adapter is post-write like claude-code — document that honestly).
- The registration mechanism (settings file? a hooks JSON in a known dir?
  project-local vs global).
- The stdin payload shape (tool name, file path, proposed content / diff).
- The block semantics (exit code? a JSON decision object?).

### Build
1. Model the adapter on the closest existing one: if Cursor registers via a
   settings/JSON-hook file, mirror **claude-code** (`JsonHookSpec` in the adapter
   registry); if via a plugin file, mirror **pi/opencode** (`PluginSpec`). The
   review notes it "likely fits `JsonHookSpec` with a new hooks.json dialect."
2. The hook script: parse the payload, produce the proposed content, call
   `ironlint check --file <path> --content -`, and map exit codes to Cursor's
   block/allow semantics — including exit 3 fail-open/`IRONLINT_FAIL_CLOSED_ON_
   INTERNAL` and exit 4 untrusted (per Phase 3 Tasks 3.2), consistently with the
   other adapters.
3. Register in `adapter/registry.rs` with detection (`~/.cursor` dir, mirroring
   how claude-code detects `~/.claude`), install/uninstall, the shared
   `ironlint-config` skill, and a version-stamped sidecar (mirror the existing
   adapters exactly).
4. `init` auto-detects and wires it; `doctor` reports its status row.
5. Write a test suite mirroring `adapters/pi/` or `adapters/opencode/` (canned
   payloads → asserted allow/block across exit 0/2/3/4). Add it to CI.
6. `docs/adapters/cursor.md` + add Cursor to the README adapter list and the
   badge.

### Verify
- [ ] `ironlint init --harness cursor --dry-run` (scratch dir) renders a correct
      plan; a real install wires a hook Cursor actually fires.
- [ ] Contract tests pass across all exit codes.
- [ ] `doctor` shows a cursor row.
- [ ] Standard Verification.

### Done when
Cursor is a first-class supported harness: detected, wired by `init`, gating via
the ABI, tested in CI, and documented — with its pre/post-write reality stated
honestly.

---

## Task 4.2 — Ship a GitHub Action (the CI round-trip)  [Finding M2]

- **Severity:** Blocker (for the "local CI" promise) · **Effort:** S/M
  · **Depends on:** none
- **Files:** new `action.yml` (composite action) at the repo root or a dedicated
  `ironlint-action/` path; `docs/operating/ci.md`.

### Why
The pitch is "local CI," but without a CI backstop the gate is advisory — any
commit from an unhooked surface (plain git, vim, an un-adapted editor) lands
unchecked. Teams expect to run the **same `.ironlint.yml`** in CI on the PR diff.
The friction to solve: CI must `ironlint trust` the config before `check`.

### Build
1. A **composite GitHub Action** (`action.yml`) that:
   - Installs a pinned IronLint binary (download the release artifact for the
     runner OS, verify checksum).
   - Runs `ironlint trust` on the repo config (in CI, the checkout *is* the
     trusted source; document this clearly — trust-in-CI means "this pipeline
     vouches for the committed config").
   - Computes the changed files (`git diff --name-only origin/${{ base }}...` or
     a proper diff) and runs `ironlint check --diff <diff-file>` (or per-file),
     failing the job on exit 2.
   - Passes through `IRONLINT_FAIL_CLOSED_ON_INTERNAL` so CI can be strict.
2. Inputs: config path, base ref, fail-closed toggle. Sensible defaults.
3. `docs/operating/ci.md`: a copy-paste workflow using the action, plus a
   **GitLab / generic-runner** equivalent (raw commands), and a short note on the
   trust-in-CI model. Add a monorepo example.
4. (Optional) publish to the GitHub Marketplace once stable.

### Verify
- [ ] The action runs in a test workflow on a branch: installs, trusts, checks a
      PR diff, and fails on a seeded violation / passes on a clean diff.
- [ ] `docs/operating/ci.md` exists with GitHub + GitLab recipes.

### Done when
A team can run the same checks in CI with a few lines of workflow YAML, closing
the local→CI loop, with the trust-in-CI step handled and documented.

---

## Task 4.3 — `init` installs a chain-safe git `pre-commit` hook; add `check --staged`  [Finding E5]

- **Severity:** Major · **Effort:** M · **Depends on:** none
- **Files:** `crates/ironlint-cli/src/commands/init/*` (hook install),
  `crates/ironlint-cli/src/cli.rs` + `commands/check.rs` (add `--staged`),
  `crates/ironlint-core/src/adapter/*` if the git-hook install belongs there,
  `docs/` (document it), tests.

### Why
The `on: [pre-commit]` lifecycle is marketed hard (the whole "vs lefthook"
pitch) and taught in the shipped skill, but **nothing installs a
`.git/hooks/pre-commit`** — five reviewers independently hit this. A user
configures pre-commit checks and nothing fires, with no warning. This makes a
core feature effectively vaporware.

### Build
1. **`check --staged` sugar:** add a `--staged` flag to `check` that enumerates
   staged files (`git diff --cached --name-only`, respecting renames — see Task
   4.4 for reading staged *content*) and runs the `pre-commit` lifecycle over
   them (`--event pre-commit`). This replaces the awkward `git diff --cached > f
   && ironlint check --diff f --event pre-commit` incantation.
2. **Chain-safe git-hook installer:** `init` offers to write `.git/hooks/pre-
   commit` (or compose with `core.hooksPath` if set). It must be **chain-safe** —
   preserve any existing hook (husky/lefthook): if a foreign hook exists, either
   append an ironlint invocation with clear markers or refuse-with-instructions
   rather than clobbering. The repo already has foreign-safe hook-composition
   logic for editor settings (`init` preserves foreign hooks in settings.json) —
   reuse that discipline. The installed hook body is minimal: `exec ironlint
   check --staged`.
3. Respect `--global`/`--dry-run`/`--yes`/`--uninstall` consistently with the
   other init flows.
4. **`doctor`:** warn when the resolved config declares any `on: [pre-commit]`
   check but no git pre-commit hook invoking ironlint is installed.

### Step — tests
- Failing test first for `check --staged` (stage a violating file, assert exit 2).
- Init test: in a temp git repo, `init` writes a `.git/hooks/pre-commit` that is
  executable and invokes `ironlint check --staged`; a **pre-existing** hook is
  preserved (chain-safety asserted).
- Doctor test: pre-commit check present + no hook → warn.

### Verify
- [ ] `check --staged` works; `init` installs a chain-safe hook; doctor warns on
      the gap.
- [ ] Existing hooks are never clobbered (asserted).
- [ ] Standard Verification.

### Done when
`ironlint init` can wire a real, chain-safe git `pre-commit` hook, `check
--staged` exists, and `doctor` flags a configured-but-unwired pre-commit.

---

## Task 4.4 — pre-commit checks read staged content, not the worktree  [Finding R2]

- **Severity:** Major · **Effort:** M/L · **Depends on:** Task 4.3
- **Files:** `crates/ironlint-core/src/runner.rs` (pre-commit dispatch),
  `crates/ironlint-cli/src/commands/check.rs` (`--staged` path).

### Why
The classic pre-commit hole: checks read **on-disk (worktree)** content, but with
`git add -p` the **staged** content differs. So a check can approve content that
is not being committed (violations ride in), or block on unstaged worktree noise.
Now that Task 4.3 makes pre-commit actually fire, this correctness gap matters.

### Build
1. For the pre-commit lifecycle, materialize each staged file's **index** content
   via `git show :<path>` (the staged blob) rather than reading the worktree.
   Extend the `$IRONLINT_TMPFILE` machinery (it already materializes proposed
   content to a temp sibling) to write the staged bytes, so checks that read a
   file see the staged version.
2. For `pre-commit`, `$IRONLINT_FILES` should list the staged set; if a check
   reads content, it must get staged content. Keep the write-lifecycle behavior
   (stdin = proposed content) unchanged.
3. Handle deletions/renames in the staged set sensibly (a deleted file has no
   content to check; a rename's new path is what's staged).

### Step — tests
- Failing test first: stage a **clean** version of a file while the worktree has a
  **violating** unstaged edit; assert the pre-commit check **passes** (reads
  staged, not worktree). And the inverse: stage a violating version, worktree
  clean → check **blocks**. Today it reads the worktree and gets this backwards.

### Verify
- [ ] Both staged-vs-worktree tests pass.
- [ ] Existing write-lifecycle tests unchanged.
- [ ] Standard Verification.

### Done when
pre-commit checks evaluate the staged blob, matching what `git commit` will
actually record.

---

## Task 4.5 — Distribution: Homebrew tap, crates.io, npm wrapper  [Finding M3]

- **Severity:** Major · **Effort:** S each · **Depends on:** none
- **Files:** `Cargo.toml` (`[workspace.metadata.dist]` installers, crate
  metadata for crates.io), release workflow, a new npm wrapper package dir,
  README install section.
- **Type:** config/release (verify by a real release dry-run)

### Why
The agent-CLI audience is brew/npm-native; teams standardize via brew/npm, not
`curl | sh`. Not being on crates.io also means `cargo binstall` cannot resolve
IronLint. cargo-dist generates a Homebrew tap nearly for free.

### Build (each sub-item is independent; do any subset)
1. **Homebrew:** enable the `homebrew` installer in `[workspace.metadata.dist]`
   `installers` and configure a tap repo (cargo-dist docs: `tap = "owner/
   homebrew-tap"`). Regenerate the release workflow (`dist generate`) and confirm
   it publishes a formula on release.
2. **crates.io:** ensure `ironlint-cli` (and `ironlint-core`) have the metadata
   crates.io requires (`description`, `license`, `repository`, `readme`,
   `keywords`, `categories`) and `cargo publish --dry-run` succeeds for both.
   Publish `ironlint-core` then `ironlint-cli`. This unlocks `cargo binstall`.
3. **npm wrapper:** a thin package whose `postinstall` downloads the correct dist
   artifact for the platform and puts `ironlint` on the path (model it on how
   tools like `esbuild`/`@biomejs/biome` ship platform binaries via npm). Keep it
   minimal.
4. Update the README Install section with brew, `cargo binstall`, and npm paths.

### Verify
- [ ] `cargo publish --dry-run -p ironlint-core` and `-p ironlint-cli` succeed.
- [ ] `dist plan` shows the homebrew installer.
- [ ] The npm wrapper installs the binary on a clean machine/container.
- [ ] README lists the new channels.

### Done when
IronLint is installable via Homebrew, `cargo install`/`cargo binstall`, and npm —
not just `curl | sh`.

---

## Task 4.6 — Parallelize check dispatch; add a verdict cache  [Finding P5]

- **Severity:** Major · **Effort:** M/L · **Depends on:** Task 2.1 (process groups)
- **Files:** `crates/ironlint-core/src/runner.rs` (the sequential dispatch loop,
  anchor: the per-check loop around line 578+; `resolve_timeout`; telemetry
  ordering).

### Why
Every matching check runs **sequentially** on every edit, and nothing is cached —
the agent blocks on the **sum** of matching checks' cold starts, 10–100× more
often than a human saves. lefthook ships `parallel: true`; trunk built on a
daemon + caching. **Must come after Task 2.1** — parallel checks without
process-group kill means N orphans at once on a timeout, and parallel `cargo`-
family checks deadlock on their own target lock.

### Build
1. **Parallel dispatch:** use `rayon` (check whether it is already a dep; add if
   not) to run the matching checks concurrently. Preserve determinism in the
   **output and telemetry order** (collect results, then fold/log in a stable
   order — do not interleave stdout). The verdict fold must be order-independent
   (Block still beats Internal regardless of completion order — the precedence
   logic in `verdict.rs` already handles this; confirm).
2. **Guard rails:** document/handle the `cargo`-family lock caveat — checks that
   touch a shared build lock cannot truly run in parallel; a note in the docs is
   the minimum. Do not exceed a sane concurrency cap.
3. **Verdict cache (opt-in):** key a cache on `(check-id, content-hash, config-
   hash)`; on a cache hit, skip the spawn and reuse the prior verdict. Store it
   somewhere ephemeral (e.g. under `.ironlint/cache/`, gitignored like the log).
   Make it **opt-in** or safely invalidated — a stale cache that returns a pass
   for changed content would be a silent bypass, so the content-hash keying must
   be exact. Default off if there's any doubt.

### Step — tests
- Failing/behavioral test: N independent checks matching one file all run and the
  aggregate verdict + telemetry order is stable across runs (parallelism must not
  change the verdict). Add a cache hit/miss test proving a changed content-hash
  misses.

### Verify
- [ ] Parallel dispatch produces identical verdicts to sequential (test).
- [ ] Timeout still kills process groups cleanly under parallelism (Task 2.1).
- [ ] Cache (if enabled) never returns a stale pass for changed content.
- [ ] Standard Verification.

### Done when
Checks matching one edit run concurrently with stable output, and an optional
content-keyed verdict cache avoids redundant spawns — without weakening the gate.

---

## Task 4.7 — `extends` from git URLs (shareable check packs)  [Finding M4]

- **Severity:** Major · **Effort:** L · **Depends on:** Task 3.3 (hashing
  referenced files — the same trust discipline applies to fetched packs)
- **Files:** `crates/ironlint-core/src/config/extends.rs`, `trust.rs`, new cache
  dir handling, docs.

### Why
pre-commit's ecosystem flywheel was `repo:` git URLs + `rev:` pins — thousands of
shared hook repos made every config a one-liner and every published repo an
acquisition channel. IronLint's `extends:` is **local-path-only**, so every team
hand-writes checks. The trust store already hashes the extends closure, so remote
packs fit the security model — and the govern spec already imagines `extends:
ironlint-govern:baseline`, so this is a prerequisite for that future.

### Build
1. Extend `extends:` to accept a **git-URL form pinned by SHA**, e.g.
   `extends: ["github:org/repo@<40-char-sha>/path/to/base.yml"]`. **Require the
   pin** — never resolve a floating ref (a moving `extends` target would be a
   silent policy change and a supply-chain hole).
2. Fetch to a **content-addressed local cache** (e.g. `~/.cache/ironlint/packs/
   <sha>/`). Resolve the extends closure from the cache.
3. **Trust integration:** the fetched pack's bytes must be part of the trust hash
   (reuse the Task 3.3 machinery). A fetched pack is untrusted until blessed, and
   changing the pinned SHA changes the hash → re-bless required. This is the
   security property that makes remote packs safe.
4. Offline behavior: if the pinned SHA is already cached, work offline; only fetch
   on a cache miss. Clear errors on fetch failure.
5. Docs: a "check packs" page explaining the syntax, pinning, trust, and a
   curated starter pack or two.

### Step — tests
- Resolve an `extends` from a local "fake remote" (a git repo in a temp dir, or a
  cache pre-populated by the test) pinned by SHA; assert the checks merge and the
  trust hash covers the pack bytes; assert changing the pack content (new SHA)
  requires re-bless.

### Verify
- [ ] A SHA-pinned remote `extends` resolves, merges (local-wins), and is
      trust-covered.
- [ ] A floating (unpinned) remote ref is rejected with a clear error.
- [ ] Standard Verification.

### Done when
Teams can `extends:` a SHA-pinned remote check pack that is fetched, cached, and
folded into the trust hash — enabling shareable policy without weakening trust.
