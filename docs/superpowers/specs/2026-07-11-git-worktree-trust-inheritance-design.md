# Git worktree trust inheritance

**Date:** 2026-07-11
**Status:** Approved design / implementation handoff
**Scope:** `ironlint-core::trust`, `ironlint trust` summary, trust documentation, and trust tests. No adapter changes.

## Problem

IronLint intentionally requires a human to bless the policy it will execute.
Today that blessing is bound to a physical checkout twice:

1. `TrustStore.entries` is keyed by the canonical absolute path of the root
   config.
2. `compute_hash` labels every config and `.ironlint/scripts/` blob with its
   canonical absolute path before hashing it.

Consequently, byte-identical policy in a linked Git worktree has both a
different store key and a different digest. A human must run `ironlint trust`
again for every worktree, even though they already reviewed the same policy in
the same repository.

## Goal

One human blessing must cover an unchanged, fully in-worktree policy in every
linked worktree of the same local Git repository. Creating a worktree,
switching branches, or editing ordinary source code must not require another
`ironlint trust` invocation.

This is convenience around an existing approval, not automatic approval:

- Any change to a covered config or covered script still revokes trust.
- `check` never writes or upgrades the trust store.
- `ironlint trust` remains the only operation that grants trust, and adapter
  Bash gates continue to prevent an agent from invoking it.

## Non-goals

- Do not trust identical policy in a separate clone, copied directory, or
  unrelated repository.
- Do not make policy trust content-addressed globally.
- Do not share a policy whose resolved `extends:` closure or covered scripts
  escape the Git worktree. Those configurations retain the existing
  path-specific behavior.
- Do not change the `check` exit-code contract or adapter handling. In
  particular, an untrusted policy remains exit `4`.
- Do not require Git to be installed or execute a `git` binary as part of the
  trust boundary. Nonstandard or unreadable Git metadata simply disables
  worktree inheritance; normal exact-path trust still works.
- Do not make repository relocation portable. The common Git directory stays
  part of the identity, consistent with the current path-bound model.

## Definitions

**Direct trust** is the existing entry in `TrustStore.entries`, keyed by the
canonical absolute config path and holding the current absolute-path-framed
digest.

**Worktree scope** is the tuple:

```text
(canonical Git common directory, config path relative to the worktree root)
```

For example, a primary checkout and its linked worktree may have different
roots but share:

```text
common directory: /repos/acme/.git
config relative path: .ironlint.yml
```

**Worktree policy hash** is a SHA-256 over exactly the present trust surface,
but labels each config and script using its normalized path relative to the
worktree root. It is therefore stable across linked worktrees with the same
policy and remains sensitive to every covered file.

**Eligible policy** means the root config, every resolved `extends:` config,
and all participating `.ironlint/scripts/` directories are under the same
canonical worktree root. A config symlinked outside the root or an external
`extends:` target is ineligible for worktree inheritance.

## Architecture

### 1. Discover Git worktree identity without running Git

Add a private `trust::worktree` helper (prefer
`crates/ironlint-core/src/trust/worktree.rs`, declared from `trust.rs`). It
receives the canonical root config path and returns `Option<WorktreeScope>`.
It must use filesystem metadata only; it must not shell out to `git`, whose
path and environment could be influenced by the agent process.

Discovery algorithm:

1. Starting at the canonical config's parent, walk upward to the nearest
   directory containing `.git`. That directory is the candidate worktree root.
2. Read `.git` without following unusual entries:
   - If it is a directory, it is the primary worktree's Git directory.
   - If it is a regular file, parse the single `gitdir: <path>` record,
     resolve relative paths against the worktree root, and canonicalize the
     resulting Git directory.
3. A `.git` directory is the primary-worktree form; its Git directory is the
   common directory. A `.git` file is eligible only as the linked-worktree
   form: its resolved Git directory must contain `commondir`, whose canonical
   value is the common Git directory.
4. For the linked-worktree form, require the Git directory to live below
   `<common-dir>/worktrees/`, then validate its reciprocal `gitdir` file points
   back to the `.git` file found at the worktree root. This prevents a loose
   `.git` file from claiming a trusted family without matching Git's standard
   linked-worktree layout.
5. Canonicalize the worktree root, derive the config path relative to it, and
   normalize separators to `/` for store stability.

If any step fails, is malformed, is a symlink/non-regular metadata file, or
places the config outside the root, return `None`. This is not a config parse
error: it merely means no shared-worktree fallback is available. In particular,
a bare `.git` file that merely points at another Git directory is not enough to
join a trust family.

The primary worktree is eligible to create a worktree-family entry even before
it has any linked siblings. That lets a later `git worktree add` inherit the
human's existing approval.

### 2. Compute a worktree-relative policy hash

Keep `compute_hash` unchanged for direct trust. Add a private
`compute_worktree_hash(config_path, scope)` that reuses the existing
`extends::resolve_paths`, `.ironlint/scripts/` enumeration, symlink refusal,
sorting, and length-prefixed `hash_entry` framing.

Its only semantic difference is the labels:

```text
config\0<path-relative-to-worktree-root>
scripts\0<path-relative-to-worktree-root>
```

The labels must include the distinct `config`/`scripts` prefix, and all paths
must use `/`. Before hashing, verify every resolved config is under the root.
The scripts directory for each resolved config is also required to be under the
root. Any escape makes the policy ineligible rather than silently omitting a
file.

This preserves the important properties of the current hash:

- config bytes and every covered script byte are included;
- ordering is deterministic;
- a file cannot be relabeled or concatenated ambiguously; and
- external script behavior remains the documented existing limitation, not a
  new trust exception.

### 3. Extend, do not replace, the trust store

Bump `TRUST_STORE_VERSION` from `1` to `2`. Preserve `entries` unchanged and
add a serde-defaulted nested map:

```rust
pub struct TrustStore {
    pub version: u32,
    pub entries: BTreeMap<String, TrustEntry>,
    pub worktree_entries: BTreeMap<String, BTreeMap<String, TrustEntry>>,
}
```

The outer key is the canonical Git common directory; the inner key is the
normalized config-relative path. The value remains `TrustEntry { hash,
blessed_at }`, where `hash` is the worktree-relative policy hash.

An existing version-1 JSON file deserializes with an empty `worktree_entries`
map. Its direct entries remain authoritative and valid exactly as before.
Existing store locking, corrupt-store recovery for `trust`, atomic writes, and
fail-closed reads for `check` are unchanged.

### 4. Blessing behavior

`bless_in` must continue to:

1. parse and validate the full `extends:` closure;
2. calculate the direct hash and canonical direct key; and
3. atomically update the direct entry under the existing lock.

When `WorktreeScope::discover` succeeds and the policy is eligible, it must
also calculate the worktree hash and update the corresponding
`worktree_entries[common_dir][config_rel]` entry in the **same** locked,
atomic write. If discovery or eligibility fails, blessing still succeeds with
only the direct entry.

`ironlint init` needs no special branch: it already calls the normal blessing
path, so a freshly initialized Git project gets the worktree-family entry
automatically.

Extend `BlessedSummary` and `ironlint trust` output with one additive scope
line, without removing or rewording existing output:

```text
  scope: linked worktrees
```

For an ineligible/non-Git config, print:

```text
  scope: this config path
```

This makes the broader approval visible to the human who grants it.

## Verification behavior

`check_trust_in` keeps its current first steps and outcome split:

1. Compute the direct hash. A failure remains `TrustOutcome::Unverifiable`,
   which the CLI maps to exit `1`.
2. Read the store. A corrupt or unreadable store remains
   `TrustOutcome::Untrusted`, which maps to exit `4`.
3. If `entries[canonical_config_path]` equals the direct hash, return
   `Trusted` exactly as today.

Only after a direct miss does it attempt inherited trust:

4. Discover the current worktree scope and calculate its worktree hash. If
   either is unavailable, skip inheritance and return the normal untrusted
   result; do not turn this into exit `1`.
5. Look up `worktree_entries[common_dir][config_rel]`. A matching hash returns
   `Trusted`.
6. If no version-2 entry matches, try the read-only legacy migration fallback
   below.
7. Otherwise return the existing fixed untrusted error:
   `config/scripts not trusted — review and run \`ironlint trust\``.

No successful inherited check writes a direct entry, a worktree entry, a
timestamp, or any other store state.

### Legacy migration fallback

The fallback eliminates a one-time re-trust after upgrading.

For each direct entry in the old `entries` map, treat it as a candidate only
when all of the following are true:

1. Its stored config path still exists and yields an eligible worktree scope.
2. That scope has the same canonical common Git directory and config-relative
   path as the current scope.
3. Recomputing the candidate's existing direct hash still equals its stored
   entry. This proves the previously blessed source policy has not changed.
4. Recomputing the candidate's worktree policy hash equals the current
   worktree policy hash.

Any one valid candidate returns `Trusted`. Missing paths, stale entries,
malformed candidate metadata, or candidate hash errors are ignored and do not
change the current config's trust outcome. The fallback is read-only and must
not lazily add a version-2 entry.

After a human next runs `ironlint trust` in any eligible worktree, a durable
version-2 family entry exists and the original legacy checkout may disappear
without affecting later sibling worktrees.

## Required behavior and failures

| Situation | Result |
| --- | --- |
| Unchanged linked worktree after a new blessing | Trusted via `worktree_entries`; checks run normally. |
| Unchanged linked worktree after upgrade with only an old direct entry | Trusted via verified, read-only legacy fallback. |
| Policy config, inherited config, or covered script changes | No hash match; exit `4`. |
| Ordinary application/source change outside policy surface | Still trusted. |
| Separate clone with identical files | Different common directory; exit `4` unless directly trusted. |
| External `extends:` config or config/scripts outside root | No inheritance; direct-path trust behavior remains. |
| No Git repo or malformed/unusual Git metadata | No inheritance; direct-path trust behavior remains. |
| Original legacy worktree deleted before an upgrade re-bless | Legacy fallback cannot prove it; one human trust in any surviving eligible tree creates the durable family entry. |

## Code and documentation changes

- `crates/ironlint-core/src/trust.rs`
  - add the store field/version, shared-hash dispatch, blessing write, lookup
    order, and legacy fallback;
  - retain the public `check_trust*`, `bless*`, and `ensure_trusted*` contracts.
- `crates/ironlint-core/src/trust/worktree.rs`
  - private Git metadata discovery, eligibility checks, normalized relative
    path helpers, and focused unit tests.
- `crates/ironlint-cli/src/commands/trust.rs`
  - render the additive scope line.
- `docs/security/trust.md`
  - document linked-worktree inheritance, its in-tree-only boundary, and the
    fact that untrusted `check` exits `4` (the document currently says `1`).
- Relevant adapter documentation
  - correct any remaining reference to untrusted configs failing open or using
    exit `1`; adapter runtime behavior itself does not change.

Keep cognitive complexity under the repository cap by isolating discovery,
hashing, version-2 lookup, and legacy-candidate validation into small private
functions rather than accumulating branches in `check_trust_in`.

## Test plan and acceptance criteria

### Unit coverage

- Detect a primary `.git` directory and a valid linked-worktree `.git` file /
  `commondir` / reciprocal `gitdir` layout.
- Refuse malformed, missing, symlinked, or inconsistent metadata for shared
  inheritance while allowing direct trust to continue.
- Normalize relative paths deterministically, including nested custom configs.
- Produce the same worktree hash for equivalent roots and a different hash for
  each changed config, inherited config, or covered script.
- Refuse a closure that escapes the worktree root.
- Deserialize a version-1 store with no worktree field.
- Verify legacy candidate validation ignores stale/deleted/broken entries and
  accepts only an unchanged sibling in the same scope.

### Integration and CLI coverage

Use a real temporary Git repository and `git worktree add` for the primary
path. The tests must prove behavior through real linked-worktree metadata, not
only a hand-written directory fixture.

1. Bless the primary tree once; `check` in an unchanged sibling runs a known
   blocking check and exits `2`, proving it executed rather than merely
   bypassing trust.
2. Create a version-1-style direct-only entry in the primary tree; the sibling
   succeeds through the legacy fallback and the store bytes do not change.
3. Modify the sibling root config, a config in its `extends:` closure, and a
   covered script in separate tests; each `check` exits `4`.
4. Create another independent repository with identical policy bytes; it does
   not inherit trust.
5. Verify a non-Git project and an external-extends project retain their
   exact-path behavior.
6. Verify `ironlint trust` in an eligible Git tree prints the new scope line
   and writes both entries; verify its existing hash/check/script summary is
   still present.
7. Exercise an explicitly selected nested config and a nested current working
   directory; scope identity must derive from the config's Git worktree, never
   from the caller's current directory.
8. Retain all existing direct-trust, corrupt-store, concurrency, extends,
   adapter exit-4, and Bash self-trust tests.

The changed Rust files must meet the repository's per-file region-coverage
gate and clippy cognitive-complexity cap.

## Handoff checklist

An implementer is done when all of these statements are true:

- A human never needs to run `ironlint trust` merely because they created a
  linked worktree.
- A user upgrading with a still-present trusted source worktree gets the same
  convenience without an extra command.
- No unrelated clone or policy change obtains trust.
- `check` remains read-only; only a human-initiated `trust` grants approval.
- Exact-path trust remains the first lookup and continues to work everywhere
  worktree sharing is unavailable.
- Exit codes and adapter contracts are unchanged.
