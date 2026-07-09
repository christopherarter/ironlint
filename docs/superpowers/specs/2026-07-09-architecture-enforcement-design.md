# Architecture enforcement — language-agnostic layer/dependency-direction rules

**Status:** Design proposal, not yet implemented.
**Date:** 2026-07-09
**Type:** New capability (native ironlint-core module + `ironlint arch` subcommand + declarative `architecture:` config sugar).

## TL;DR

ironlint gains a first-party **architecture enforcement** capability: declare named
layers (glob sets) and directional rules between them ("`presentation` may import
`domain` and `data`; `data` may import nothing"), and ironlint builds a
dependency graph from the codebase's imports and blocks any write/commit that
introduces a forbidden edge. It is **language-agnostic** in mechanism and
**per-language** in import resolution — v1 ships TypeScript/JavaScript, Rust,
Python, Go, and PHP.

The engine is an **`ironlint arch` subcommand** — a standalone capability that
does the graph work and exits on ironlint's existing contract (`0` Pass / `2`
Block / `3` InternalError). The user-facing `architecture:` config block is
**syntactic sugar that lowers to a synthetic `Check`** whose `run` shells out to
that subcommand. The runner never learns that architecture enforcement exists —
it still only ever sees checks. This preserves the 0.4 invariant ("no per-rule
engines, checks own their verdict") while delivering a structured, ergonomic
rule surface.

The differentiator over every existing structural linter (ls-lint, alint,
structlint, chous, kekkai-structure-lint): **proposed-content awareness at
write-time.** Because the engine runs inside ironlint's `write` lifecycle, it
sees the agent's proposed file *before it lands* and can block a forbidden
import before the write happens — not after commit. ls-lint and its peers scan
on-disk state only; they cannot see a not-yet-written file.

## Why it matters

AI coding agents write a lot of code fast, and the class of drift they produce
most reliably is **architectural**: a component that imports a database driver,
a utility that reaches into a feature module, a data-layer file that depends on
presentation. These are not style violations (biome/eslint catch those) and not
name violations (ls-lint catches those) — they are **dependency-direction**
violations, and no tool in ironlint's orbit catches them at write-time today.

The result without enforcement: an agent can introduce a layer violation in
seconds that a human reviewer catches in a PR hours later, by which point the
agent has built further on the broken foundation. Catching it at write-time —
before the file lands — is the single highest-leverage governance ironlint can
add for agent-driven codebases.

## Design principles

1. **The runner stays pure.** Architecture enforcement lowers to a check. No
   new dispatch branch in `run_one_check`, no `ArchitectureEngine` trait, no
   per-rule kind. The 0.4 invariant holds.
2. **The engine is independent.** `ironlint arch` is a real subcommand usable
   with no `.ironlint.yml` (CI, scripts, debugging). Remove the sugar and the
   capability is intact. This is the property that makes the sugar *sugar*
   rather than an essential special case.
3. **Language-agnostic mechanism, per-language resolution.** The graph (nodes =
   files, edges = imports) and the rule evaluator are language-agnostic. Only
   import extraction (Layer 1) and import resolution (Layer 2) are per-language,
   behind traits so each language is additive.
4. **Honest about per-write's reach.** Per-write checks the **outgoing** edges of
   the proposed file. It cannot check incoming edges (other files importing the
   proposed file) because the proposed file isn't on disk yet. Pre-commit/sweep
   checks the whole graph, both directions. This split is intrinsic, not a
   limitation to "fix."
5. **Structural only.** The tool reasons about files as nodes and imports as
   edges. It never judges code correctness, style, or types. That line is the
   guardrail against becoming eslint.

## The three-layer cake

```
┌─────────────────────────────────────────────────────────┐
│  Layer 3 — Policy evaluator (language-agnostic)          │
│  classify each file into a layer; for each edge, check   │
│  the importer's `may_import` against the target's layer.  │
│  Collect violations → Block/Pass.                         │
├─────────────────────────────────────────────────────────┤
│  Layer 2 — Import resolver + graph (per-language)        │
│  map each import source string to an absolute file path. │
│  Unresolved → dropped edge (external dep, stdlib — not    │
│  architectural). Build file→file edges.                   │
├─────────────────────────────────────────────────────────┤
│  Layer 1 — Import extractor (per-language, tree-sitter)   │
│  parse each file; pull import source strings out of the   │
│  AST (e.g. "./components/X", "@/foo", "crate::bar").      │
└─────────────────────────────────────────────────────────┘
```

### Layer 1 — Import extraction (tree-sitter, embedded)

**Decision:** embed [tree-sitter](https://tree-sitter.github.io/) as Rust crates.
Not ast-grep. Rationale:

- ast-grep is a **single-file pattern matcher**. Its strength is structural
  *search/rewrite* within one file. Import extraction needs none of that power —
  it needs to walk an AST and pull out source strings, which is a trivial
  tree-sitter query.
- ast-grep runs as a **subprocess** (binary on PATH, YAML rule files per
  language). tree-sitter is an **in-process Rust crate** — no subprocess, no
  per-scan spawn, no YAML rule files to ship. For per-write (which fires on
  every agent write), the subprocess overhead is a real cost; in-process is the
  right call.
- ast-grep's coverage (~25 languages) exceeds what we need for v1, but the
  languages we *do* need are all well-supported by tree-sitter grammars.
- ast-grep **cannot do Layers 2 or 3** — it has no import resolver, no graph,
  no cross-file model. The features that justify this build are exactly the ones
  ast-grep doesn't have. Bringing it in would add a dependency for the one layer
  where it's overkill, while we build the other two ourselves anyway.

Workspace deps: `tree-sitter`, `tree-sitter-typescript`, `tree-sitter-javascript`,
`tree-sitter-rust`, `tree-sitter-python`, `tree-sitter-go`, `tree-sitter-php`.
Each grammar is a crate; loading is lazy per detected language.

**Trait:**

```rust
/// Extract import source strings from a parsed file.
///
/// One impl per language. The query is near-identical across languages —
/// the per-language work is *which* AST node kinds constitute an import
/// (e.g. TS `import_statement` / `call_expression` for dynamic import;
/// Rust `use_declaration`; Python `import_statement` / `import_from_statement`).
trait ImportExtractor {
    /// The tree-sitter language for this extractor.
    fn language(&self) -> tree_sitter::Language;
    /// Source strings as written (pre-resolution): "./foo", "@/bar", "crate::baz".
    /// Order preserved for stable violation output.
    fn extract(&self, tree: &tree_sitter::Tree, source: &[u8]) -> Vec<ImportSource>;
}

/// A raw import source string + its span (for violation messages).
struct ImportSource {
    spec: String,       // as written, e.g. "./components/UserCard"
    line: usize,        // 1-indexed, for "auth.ts:12 imports from forbidden layer"
}
```

### Layer 2 — Import resolution + graph (per-language)

This is the hard, language-specific part. The **mechanism** is shared (join +
try extensions + cache); the **rules** per language are not.

**Trait:**

```rust
/// Resolve a raw import source string to an absolute file path on disk.
///
/// Returns `None` for anything that isn't a project-internal file: external
/// packages, stdlib, bare specifiers with no resolvable target. Unresolved
/// imports are dropped from the graph — they are not architectural edges.
///
/// One impl per language. This is where language module-system knowledge lives.
trait Resolver {
    /// Resolve `spec` as imported from `importer` (the file containing the import).
    /// `root` is the project root ($IRONLINT_ROOT).
    fn resolve(
        &self,
        spec: &str,
        importer: &Path,
        root: &Path,
    ) -> Option<PathBuf>;
}
```

**Per-language resolver scope (v1):**

| Language | Resolver concerns | Complexity |
|---|---|---|
| TypeScript / JavaScript | `tsconfig`/`jsconfig` `paths` aliases (`@/*`), extension inference (`.ts`/`.tsx`/`.js`/`.jsx`/`.d.ts`/`/index.*`), barrel `index.*` resolution, `package.json` `exports`/`main`. | **Hardest.** Ships first — validates the trait design against the worst case. |
| Rust | `crate::`/`super::`/`self::`/`crate::*`, external crates (resolve via `Cargo.toml` `[dependencies]` — external = dropped edge), `mod` declarations. | Medium. You know it cold; validates a second impl. |
| Python | Relative `.`/`..` imports, `sys.path`/package dirs, namespace packages, `__init__.py` presence. | Medium. |
| Go | Module path → directory mapping via `go.mod` (`module github.com/x/y` → repo root), internal packages. | Lower. Convention-driven. |
| PHP | PSR-4 autoloader mapping via `composer.json`, namespace → directory. | Medium. |

**Resolution is best-effort and conservative.** An import that can't be
resolved to a project file is **dropped** (not a violation). A false drop is
acceptable; a false Block is not. The resolver never blocks — only the policy
evaluator (Layer 3) blocks, and only on resolved edges.

**The graph:**

```rust
struct DepGraph {
    /// file path → node (layer + outgoing resolved edges)
    nodes: HashMap<PathBuf, Node>,
}

struct Node {
    layer: Option<LayerId>,        // None = unlayered (no rules apply to it as importer)
    edges: Vec<Edge>,              // outgoing resolved imports
}

struct Edge {
    target: PathBuf,                // resolved absolute path
    spec: String,                  // original source string (for messages)
    line: usize,
}
```

### Layer 3 — Policy evaluator (language-agnostic)

Pure graph + rule evaluation. Classify each file into a layer (first matching
layer's globs win; unmatched = unlayered). For each edge, look up the importer's
layer and the target's layer; if the importer's `may_import` doesn't include the
target's layer, it's a violation.

```rust
struct Violation {
    importer: PathBuf,
    target: PathBuf,
    importer_layer: LayerId,
    target_layer: LayerId,
    spec: String,                  // "from './components/UserCard'"
    line: usize,
    rule: RuleRef,                 // which `may_import` rule this violated
}
```

## Config shape (the sugar)

A new top-level `architecture:` block in `.ironlint.yml`:

```yaml
architecture:
  layers:
    presentation: ["src/components/**", "src/pages/**"]
    domain:       ["src/domain/**", "src/services/**"]
    data:         ["src/data/**", "src/lib/db/**"]
  rules:
    - from: presentation
      may_import: [domain, data]
    - from: domain
      may_import: [data]
    - from: data
      may_import: []          # data imports nothing from these layers
  ignore:                       # optional, glob list — excluded from the graph
    - "**/*.test.*"
    - "**/*.spec.*"
    - "**/generated/**"
    - "**/__mocks__/**"

checks:
  biome:
    files: ["**/*.ts"]
    run: "biome check"
```

**Semantics:**

- `layers` maps a layer name → globs. A file's layer is the **first** layer whose
  globs match it (deterministic; order = YAML insertion order). A file matching
  no layer is **unlayered**: no rules apply to it *as an importer*, but it can
  still be *imported* (only the importer's rules fire). This lets you enforce
  rules over a subset of the repo (e.g. only `src/`) without classifying
  everything.
- `rules` are directional: `from X may_import [Y, Z]` means a file in layer X
  may import from layers Y and Z. Importing from a layer not in `may_import`
  blocks. Importing from an unlayered file is always allowed (unlayered files
  are outside the policy surface).
- `ignore` excludes files from the graph entirely — they neither impose rules
  (as importers) nor are subject to them (as targets). Use for tests, generated
  code, mocks. (Distinct from "unlayered": an ignored file isn't even a node.)
- A layer with no `rules` entry is allowed to import from **any** layer
  (permissive default — opt-in restriction via explicit `may_import: []`).

**Lowering:** after `extends` resolution, a lowering pass rewrites the config.
If `architecture:` is present, ironlint synthesizes a `Check` and inserts it
into `checks` under reserved id `__arch__`:

```yaml
# what the runner sees (synthesized, never hand-written)
checks:
  __arch__:
    files: ["**/*"]
    on: [write, pre-commit]
    run: |
      ironlint arch check \
        --layers "$IRONLINT_ARCH_LAYERS" \
        --root "$IRONLINT_ROOT" \
        --event "$IRONLINT_EVENT" \
        --file "$IRONLINT_FILE"
```

The `run` shells out to the `ironlint arch check` subcommand. `$IRONLINT_ARCH_LAYERS`
is a tempfile ironlint writes from the inline `architecture:` block (same
materialization pattern as `$IRONLINT_TMPFILE`). `$IRONLINT_FILE` is set on
`write` (the proposed file) and unset on `pre-commit` (whole-graph mode).

**Why `on: [write, pre-commit]`:** per-write checks the proposed file's outgoing
edges (the AI-era differentiator); pre-commit checks the whole graph both
directions (catches incoming-edge violations per-write can't see). Both fire;
no duplication (ironlint keys by check, not event).

## The `ironlint arch` subcommand surface

```
ironlint arch check [--layers <file>] [--root <dir>] [--event write|pre-commit]
                    [--file <path>] [--json]
ironlint arch graph  [--layers <file>] [--root <dir>] [--dot|--json]
ironlint arch why <path> [--layers <file>] [--root <dir>]
```

- **`arch check`** — the verdict path. Builds (or updates) the graph, evaluates
  rules, exits `0` (Pass) / `2` (Block, violations on stdout) / `3`
  (InternalError: parse failure, missing layers file, cache corruption). Maps
  onto ironlint's exit contract exactly — no new exit codes.
  - `--event write --file <path>`: incremental mode. Evaluate only the proposed
    file's outgoing edges against the cached graph. The proposed file's content
    is read from stdin (the check ABI already feeds proposed content on stdin
    for `write`).
  - `--event pre-commit` (no `--file`): whole-graph mode. Evaluate all edges,
    both directions.
- **`arch graph`** — emits the resolved dependency graph. `--dot` for Graphviz
  (`dot -Tsvg`), `--json` for machine consumption. Standalone debugging/CI-doc
  value. This is the payoff of independence: the graph is inspectable, not a
  black box.
- **`arch why <path>`** — "why does this file block?" Shows the violating edges
  for one file. The agent-facing debugging path: when a write blocks, the agent
  runs `ironlint arch why $IRONLINT_FILE` to understand and fix.

**Exit contract (reuses ironlint's, no new codes):**

| Exit | Meaning | `arch check` trigger |
|---|---|---|
| `0` | Pass | no violations |
| `2` | Block | ≥1 layer violation (violations on stdout, one per line) |
| `3` | InternalError | parse failure, missing/corrupt layers file, cache corruption, tree-sitter grammar load failure |

`arch graph` and `arch why` exit `0` on success / `3` on internal error (never
`2` — they're read-only inspection, not verdicts).

## The graph cache (per-write correctness)

Per-write day one makes the graph cache **essential**. Get it wrong and you
get either false passes (stale graph) or unacceptable latency (re-parse the
whole repo every write).

### The hard correctness case

A multi-file agent edit: the agent writes file A (adds an export), then writes
file B (imports from A). If the cache holds A's *pre-edit* content when
evaluating B, the resolver might fail to resolve B's import (false Block) or
resolve it to a stale path (false Pass). So the cache key must be
**content-derived, per-file**, and the engine must re-read a file when its
content changes — including changes made by *this* ironlint process in a prior
write event within the same session.

### Cache model: content-addressed node cache

```rust
struct NodeCache {
    /// file path → (content_hash, layer, resolved_imports)
    entries: HashMap<PathBuf, CachedNode>,
}

struct CachedNode {
    hash: u64,                     // content hash (xxhash, fast)
    layer: Option<LayerId>,
    edges: Vec<Edge>,              // resolved outgoing imports
}
```

**On a `write` event for proposed file F:**

1. Read F's proposed content from stdin. Hash it. Extract imports (Layer 1).
2. For each import, resolve (Layer 2) against the **cached** graph. For any
   target whose cached `hash` differs from its current on-disk content, re-parse
   and re-resolve that node before evaluating. (Detects prior-write edits within
   the session.)
3. Evaluate F's outgoing edges (Layer 3). Report violations.
4. **Do not** insert F into the cache — F isn't on disk yet. F is cached only at
   pre-commit (when it has landed).

**On a `pre-commit` event:** rebuild the cache from disk (or validate the
existing cache against current mtimes/hashes and update only changed nodes),
then evaluate the whole graph both directions.

### What per-write can and cannot check (the honest split)

- **Per-write CAN:** check the proposed file's **outgoing** edges — "does B
  import from a forbidden layer?" This is the differentiator: block the agent's
  bad import before the write lands.
- **Per-write CANNOT:** check **incoming** edges — "did adding this export break
  a rule for files that import F?" — because F isn't on disk, so no other file's
  resolved edge points to F's *new* content yet. Incoming is a pre-commit/sweep
  concern.

This split is **intrinsic** to the proposed-file-not-on-disk reality, not a
limitation to fix. The design bakes it in: per-write = outgoing; pre-commit =
whole graph. Both fire; the user gets early outgoing detection at write-time
and full bidirectional coverage at commit.

### Cache invalidation

- **Content-hash mismatch** → re-parse that node. (Catches edits within the
  session and external edits between runs.)
- **Missing file** (cached node's path no longer exists) → drop the node.
- **Cache store:** in-memory only for v1 (per-process). No on-disk cache file —
  the cache lives for the duration of one `ironlint` invocation. A long-running
  `ironlint watch` session holds it across writes; a one-shot `ironlint check`
  rebuilds each run. On-disk persistence is a fast-follow if rebuild cost bites.

### Failure mode: cache corruption

If the cache is internally inconsistent (e.g. a node's hash was computed against
content that's since changed in a way the hash didn't catch — shouldn't happen,
but defensively), the engine **fails closed**: treats it as InternalError
(exit 3), never a silent pass. A broken cache is never a pass.

## Trust

The `architecture:` block is config bytes → already folded into `compute_hash`
(`crates/ironlint-core/src/trust.rs`), same as `execution:` and `checks:`.
Editing a layer glob or rule revokes trust, exactly like editing a `run:`
string. **Zero new trust machinery.**

The `ironlint arch` subcommand is **not trust-gated** when invoked directly
(it's a read-only graph tool, like `show-resolved-config`). When invoked via the
synthesized `__arch__` check, trust is enforced at the CLI `check` layer before
the check runs — same as every other check. No special-casing.

The layers tempfile (`$IRONLINT_ARCH_LAYERS`) is derived from the trusted config
block, so it inherits trust transitively — no separate hashing of the tempfile.

## `extends` interaction

`merge_inherited` (`config/extends.rs:80`) merges `checks` and `execution`. It
must also merge `architecture`:

- A base config defines `architecture:`; a child that doesn't inherits it.
- A child that defines `architecture:` **replaces** the base's (local wins,
  same as a colliding check id). Layer/rule merging *within* `architecture:`
  is **not** supported in v1 — it's whole-block replace. Rationale: layer
  semantics compose poorly (two configs naming a layer `domain` with different
  globs is ambiguous), and whole-block replace is predictable. Additive layer
  merging is a fast-follow if real usage demands it.

## `doctor` / `explain` / `show-resolved-config`

All free, because the capability lowers to a check:

- **`doctor`**: `check_run_path` already validates that a check's referenced
  binary exists. The synthesized check references `ironlint` (itself), so doctor
  reports nothing new for the binary. **New:** doctor should report whether
  tree-sitter grammars for the repo's detected languages loaded — a new
  `check_arch_grammars` doctor sub-check (warn if a language in the repo has no
  grammar; the engine drops those files from the graph rather than failing).
- **`explain`**: iterates `checks`; `__arch__` appears like any other check.
  Its `run` is the `ironlint arch check ...` command.
- **`show-resolved-config`**: the synthesized `__arch__` check appears in
  resolved output, showing the user what their `architecture:` block lowered to.

## Scope — what's in and out

### In scope (v1)

- Declarative `architecture:` block (layers + rules + ignore).
- `ironlint arch check|graph|why` subcommand.
- tree-sitter import extraction for **TypeScript, JavaScript, Rust, Python, Go,
  PHP** (five languages; six grammars — TS and JS use separate tree-sitter
  grammars but share one resolver).
- Per-language resolvers for the same five.
- Content-addressed in-memory graph cache.
- Per-write (outgoing) + pre-commit/sweep (whole-graph) enforcement.
- Trust via existing config-hash (no new machinery).
- `extends` whole-block replace.

### Out of scope (v1, fast-follows)

- **`--fix` (import rewriting across files).** High value (agent can apply the
  suggested fix), high risk (rewriting imports touches multiple files). v2.
- **On-disk graph cache persistence.** In-memory only for v1; persist if rebuild
  cost bites in practice.
- **Additive `architecture:` merging under `extends`.** Whole-block replace
  for v1.
- **Cross-repo graph** (monorepo-internal only).
- **Content-aware name rules** (name ↔ export consistency — the Tier-1 feature
  from the broader structural-linter brainstorm). Separate spec; this one is
  architecture enforcement only.
- **Additional languages** (Java/Kotlin, C#, Scala, Ruby, Swift). Additive via
  the trait; ship as demand warrants.

### Explicitly never

- Code-quality rules (style, types, correctness) — biome/eslint/clippy's job.
- Per-language style rules beyond import extraction.
- A plugin/WASM system. tree-sitter grammars are the extension point; a plugin
  layer is a v2+ question.

## Language sequencing

1. **TypeScript / JavaScript** — highest value (where agents write most code),
  hardest resolver (validates the trait design against the worst case). Ships
  first.
2. **Rust** — second impl, validates the resolver trait with a different module
  system. You know it cold.
3. **Python, Go, PHP** — additive, in that order.

Each language is a self-contained addition behind the `ImportExtractor` +
`Resolver` traits. The graph and evaluator don't change per language.

## Testing strategy

- **Per-language resolver tests:** fixture repos per language exercising alias
  resolution, barrel/index, relative imports, external-package dropping. These
  are the highest-risk surface — mutation-test them.
- **Graph cache correctness:** the multi-file-edit case (write A, then write B
  importing A) must resolve correctly — this is the essential correctness
  test for per-write. A surviving mutant here means a false pass.
- **Lowering tests:** `architecture:` block → synthesized `__arch__` check with
  the expected `run`/`files`/`on`. Parity with how `ls-lint:` lowering would
  work (same pattern).
- **Exit-contract tests:** `arch check` exits 0/2/3 per the contract; `arch
  graph`/`arch why` exit 0/3 (never 2).
- **Trust tests:** editing the `architecture:` block revokes trust (reuses the
  existing `editing_config_changes_hash` pattern).
- **`extends` tests:** base `architecture:` inherited by child; child
  `architecture:` replaces base's.

## Open questions (to resolve in the plan, not the spec)

1. **`$IRONLINT_ARCH_LAYERS` vs. passing the block inline.** A tempfile mirrors
   `$IRONLINT_TMPFILE` and avoids arg-length limits on large configs. Confirmed
   approach, but the tempfile lifecycle (write before check, clean after) needs
   the same `TmpFileGuard` pattern as `maybe_materialize_tmpfile`.
2. **Watch-mode cache lifetime.** `ironlint watch` is long-running; the cache
   persists across writes. Need to confirm the watch command's process model
   holds the cache (vs. spawning per-check subprocesses that lose it). If
   per-check is a fresh subprocess, the cache rebuilds every write — acceptable
   for v1, but flags the on-disk-persistence fast-follow.
3. **Unlayered-file policy.** Confirmed: unlayered files can be imported freely
  (only importer rules fire). But should an unlayered file *importing from a
  layered file* be allowed? Current design: yes (unlayered importer = no rules).
  Confirm this matches intent — the alternative ("unlayered may not import
  layered") is a different policy that some teams want.

## References

- 0.4 checks pipeline design: `specs/2026-06-28-ironlint-checks-pipeline-design.md`
- Bash-gate self-trust prevention: `docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`
- Trust enforcement (CLI check layer): `crates/ironlint-core/src/trust.rs`, `crates/ironlint-cli/src/commands/check.rs`
- `extends` merging: `crates/ironlint-core/src/config/extends.rs:80` (`merge_inherited`)
- Tmpfile materialization (the pattern `$IRONLINT_ARCH_LAYERS` reuses): `crates/ironlint-core/src/runner.rs:766` (`maybe_materialize_tmpfile`)
- Check ABI: `$IRONLINT_FILE`, `$IRONLINT_ROOT`, `$IRONLINT_EVENT`, stdin proposed content — `crates/ironlint-core/src/engine/gate.rs`
- tree-sitter: https://tree-sitter.github.io/
- ls-lint (the naming-only predecessor; this design supersedes the proxy-field approach): https://github.com/loeffel-io/ls-lint
