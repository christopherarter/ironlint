# Architecture Enforcement Implementation Plan (v1: engine + TS/JS + integration)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a first-party architecture-enforcement capability in ironlint: declare named layers and directional import rules, and ironlint blocks any write/commit that introduces a forbidden dependency edge — with per-write (outgoing) and pre-commit (whole-graph) enforcement for TypeScript/JavaScript.

**Architecture:** A three-layer engine (tree-sitter import extraction → per-language resolver → language-agnostic policy evaluator) exposed as an `ironlint arch` subcommand. A declarative `architecture:` config block lowers to a synthetic `__arch__` check whose `run` shells out to that subcommand — the runner stays pure (0.4 invariant: no per-rule engines). A content-addressed in-memory graph cache makes per-write fast and correct under multi-file edits.

**Tech Stack:** Rust, tree-sitter (+ `tree-sitter-typescript`, `tree-sitter-javascript`), clap, serde_yaml, xxhash (via `twox-hash`), existing ironlint-core/ironlint-cli crates.

**Spec:** `docs/superpowers/specs/2026-07-09-architecture-enforcement-design.md`

## Global Constraints

- Rust files under `crates/*/src/` must meet ≥80% **region** coverage (CI: `bash scripts/ci-coverage.sh`).
- Cognitive complexity per function capped at **15** (`clippy.toml`, `#![warn(clippy::cognitive_complexity)]` at each crate root). Refactor over annotate; `#[allow]` only when intrinsic — document why.
- `Cargo.lock` is committed; add new deps with `cargo build` to regenerate, then commit.
- Binary is `ironlint`, not `ironlint-cli`.
- Trust enforcement lives in the CLI `check` command, not `IronLintEngine::load`. Read-only commands do not enforce trust. `ironlint arch` is read-only when invoked directly (not trust-gated); trust applies only via the synthesized `__arch__` check at the `check` layer.
- Exit contract is locked: `0` Pass / `1` config-load error / `2` Block / `3` InternalError / `4` Untrusted. `arch check` reuses `0`/`2`/`3`; `arch graph`/`arch why` use `0`/`3` (never `2`).
- The check ABI: `$IRONLINT_FILE`, `$IRONLINT_FILES`, `$IRONLINT_ROOT`, `$IRONLINT_EVENT`, `$IRONLINT_TMPFILE`, proposed content on stdin. New var `$IRONLINT_ARCH_LAYERS` follows the same materialized-tempfile pattern.
- Structural only: files as nodes, imports as edges. Never code-quality/style/types rules.

## Scope of THIS plan

**In:** engine (extract → resolve → graph → evaluate), TS/JS resolver, content-addressed cache, `ironlint arch check|graph|why` subcommand, `architecture:` config block + lowering to `__arch__` check, `$IRONLINT_ARCH_LAYERS` materialization, `extends` whole-block replace, doctor grammar check, trust via existing hash.

**Out (follow-on plans):** Rust, Python, Go, PHP resolvers (additive behind the trait); `--fix` import rewriting; on-disk cache persistence; additive `architecture:` merging under `extends`; content-aware name rules.

---

## File Structure

### New files

- `crates/ironlint-core/src/arch/mod.rs` — module root; re-exports, `ArchConfig` parse/deserialize.
- `crates/ironlint-core/src/arch/config.rs` — `ArchConfig { layers, rules, ignore }` structs + serde + validation.
- `crates/ironlint-core/src/arch/extract.rs` — `ImportExtractor` trait + `ImportSource` struct.
- `crates/ironlint-core/src/arch/resolve.rs` — `Resolver` trait + shared resolution helpers (join, try-extensions).
- `crates/ironlint-core/src/arch/graph.rs` — `DepGraph`, `Node`, `Edge`, `LayerId`, builder.
- `crates/ironlint-core/src/arch/cache.rs` — `NodeCache`, `CachedNode`, content-hash invalidation.
- `crates/ironlint-core/src/arch/evaluate.rs` — `Violation`, `evaluate` (whole-graph) + `evaluate_outgoing` (per-file).
- `crates/ironlint-core/src/arch/engine.rs` — `ArchEngine`: ties extract→resolve→graph→cache→evaluate; the `check`/`graph`/`why` entry points.
- `crates/ironlint-core/src/arch/languages/mod.rs` — language registry: extension → extractor + resolver.
- `crates/ironlint-core/src/arch/languages/typescript.rs` — TS/JS `ImportExtractor` + `Resolver` (shared; tsconfig paths, extension inference, barrel index).
- `crates/ironlint-core/src/arch/lowering.rs` — `lower_architecture(&mut Config)`: `architecture:` block → synthetic `__arch__` check.
- `crates/ironlint-cli/src/commands/arch.rs` — `ironlint arch check|graph|why` CLI adapter.
- `crates/ironlint-core/tests/arch_*.rs` — integration tests (resolver fixtures, cache correctness, lowering, exit contract).
- `crates/ironlint-cli/tests/cli_arch.rs` — e2e CLI tests.

### Modified files

- `crates/ironlint-core/src/config/types.rs` — add `architecture: Option<ArchConfig>` field to `Config`.
- `crates/ironlint-core/src/config/extends.rs` — `merge_inherited`: merge `architecture` (whole-block, local wins).
- `crates/ironlint-core/src/config/mod.rs` — `pub mod arch;` wiring (arch lives under core, not config — but config references it).
- `crates/ironlint-core/src/lib.rs` — `pub mod arch;`.
- `crates/ironlint-core/src/runner.rs` — call `lower_architecture` after extends resolution; materialize `$IRONLINT_ARCH_LAYERS` tempfile (reuse `TmpFileGuard` pattern).
- `crates/ironlint-cli/src/cli.rs` — add `Arch` subcommand enum variant + args.
- `crates/ironlint-cli/src/commands/mod.rs` — `pub mod arch;`.
- `crates/ironlint-cli/src/main.rs` — dispatch `Command::Arch`.
- `crates/ironlint-cli/src/commands/doctor.rs` — `check_arch_grammars` sub-check.
- `crates/ironlint-core/Cargo.toml` — add `tree-sitter`, `tree-sitter-typescript`, `tree-sitter-javascript`, `twox-hash`.
- `crates/ironlint-cli/Cargo.toml` — add `ironlint-core` arch feature if feature-gated (likely not needed; arch is always-on).

---

## Task 1: Add tree-sitter dependencies and a smoke parse

**Files:**
- Modify: `crates/ironlint-core/Cargo.toml`
- Create: `crates/ironlint-core/src/arch/mod.rs`
- Modify: `crates/ironlint-core/src/lib.rs`
- Test: `crates/ironlint-core/src/arch/mod.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `arch` module exists and parses a TS string with tree-sitter (proves grammars link).

- [ ] **Step 1: Add deps to Cargo.toml**

Append to `crates/ironlint-core/Cargo.toml` `[dependencies]`:

```toml
tree-sitter = "0.23"
tree-sitter-typescript = "0.23"
tree-sitter-javascript = "0.23"
twox-hash = "1.6"
```

- [ ] **Step 2: Regenerate Cargo.lock and verify it builds**

Run: `cargo build -p ironlint-core 2>&1 | tail -20`
Expected: builds (deps resolve). Commit `Cargo.lock` if changed.

- [ ] **Step 3: Create the arch module with a smoke test**

`crates/ironlint-core/src/arch/mod.rs`:

```rust
//! Architecture enforcement: language-agnostic layer/dependency-direction rules.
//!
//! See `docs/superpowers/specs/2026-07-09-architecture-enforcement-design.md`.
//! Three layers: extract (tree-sitter) → resolve (per-language) → evaluate
//! (language-agnostic). Exposed via the `ironlint arch` subcommand and the
//! `architecture:` config block (which lowers to a synthetic `__arch__` check).

pub mod cache;
pub mod config;
pub mod engine;
pub mod evaluate;
pub mod extract;
pub mod graph;
pub mod languages;
pub mod lowering;
pub mod resolve;

#[cfg(test)]
mod tests {
    #[test]
    fn tree_sitter_parses_typescript() {
        use tree_sitter::Parser;
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .expect("load TS grammar");
        let tree = parser
            .parse("import { x } from './foo';", None)
            .expect("parse");
        assert!(tree.root_node().has_error() == false);
    }
}
```

Add to `crates/ironlint-core/src/lib.rs` (after existing `pub mod` lines):

```rust
pub mod arch;
```

- [ ] **Step 4: Create stub files so the module compiles**

Each of `cache.rs`, `config.rs`, `engine.rs`, `evaluate.rs`, `extract.rs`, `graph.rs`, `languages/mod.rs`, `lowering.rs`, `resolve.rs` — empty file with just a module doc comment for now:

```rust
//! (populated in later tasks)
```

`crates/ironlint-core/src/arch/languages.rs` is a dir: create `languages/mod.rs` with `//! (populated later)`.

- [ ] **Step 5: Run the smoke test**

Run: `cargo test -p ironlint-core arch::tests::tree_sitter_parses_typescript -- --nocapture`
Expected: PASS (proves tree-sitter + TS grammar link correctly).

- [ ] **Step 6: Commit**

```bash
git add crates/ironlint-core/Cargo.toml crates/ironlint-core/Cargo.lock \
        crates/ironlint-core/src/lib.rs crates/ironlint-core/src/arch/
git commit -m "feat(arch): add tree-sitter deps + arch module skeleton"
```

---

## Task 2: `ArchConfig` — parse and validate the `architecture:` block

**Files:**
- Create: `crates/ironlint-core/src/arch/config.rs`
- Modify: `crates/ironlint-core/src/config/types.rs` (add field to `Config`)
- Test: inline in `config.rs`

**Interfaces:**
- Consumes: nothing (leaf type).
- Produces: `ArchConfig`, `LayerDecl`, `RuleDecl` — the parsed config shape later tasks lower and evaluate.

- [ ] **Step 1: Write the failing test**

In `crates/ironlint-core/src/arch/config.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchConfig {
    pub layers: Vec<LayerDecl>,
    #[serde(default)]
    pub rules: Vec<RuleDecl>,
    #[serde(default)]
    pub ignore: Vec<String>,
}

/// A named layer: `presentation: ["src/components/**", ...]`. Parsed from a
/// YAML mapping (name → glob list), so order = insertion order (deterministic
/// first-match layer classification).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerDecl {
    pub name: String,
    pub globs: Vec<String>,
}

/// `from X may_import [Y, Z]`. A layer with no rule entry may import any layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDecl {
    pub from: String,
    pub may_import: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_arch_config() {
        let yaml = "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\nrules:\n  - from: presentation\n    may_import: [data]\nignore: [\"**/*.test.*\"]\n";
        let cfg: ArchConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.layers.len(), 2);
        assert_eq!(cfg.layers[0].name, "presentation");
        assert_eq!(cfg.rules[0].from, "presentation");
        assert_eq!(cfg.rules[0].may_import, vec!["data".to_string()]);
        assert_eq!(cfg.ignore, vec!["**/*.test.*".to_string()]);
    }

    #[test]
    fn rules_and_ignore_default_to_empty() {
        let yaml = "layers:\n  - name: x\n    globs: [\"*\"]\n";
        let cfg: ArchConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.rules.is_empty());
        assert!(cfg.ignore.is_empty());
    }

    #[test]
    fn rejects_unknown_field() {
        let yaml = "layers: []\nfoo: bar\n";
        let err = serde_yaml::from_str::<ArchConfig>(yaml).unwrap_err().to_string();
        assert!(err.contains("foo"), "{err}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ironlint-core arch::config 2>&1 | tail -10`
Expected: FAIL — `ArchConfig` not yet wired into anything (the test itself defines the type, so it should actually pass once the file compiles; the real failure is the type not existing in `types.rs` yet). If the inline test passes, proceed — the integration test comes in Step 4.

- [ ] **Step 3: Add `architecture` field to `Config`**

In `crates/ironlint-core/src/config/types.rs`, add to the `Config` struct (after `checks`):

```rust
    /// Optional architecture-enforcement block. Lowers to a synthetic
    /// `__arch__` check (see `arch::lowering`). None = no architecture rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<crate::arch::config::ArchConfig>,
```

- [ ] **Step 4: Write the integration test (config parses the block)**

In `crates/ironlint-core/src/config/types.rs` `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn parses_config_with_architecture_block() {
        let cfg: Config = serde_yaml::from_str(
            "architecture:\n  layers:\n    - name: data\n      globs: [\"src/data/**\"]\n  rules:\n    - from: data\n      may_import: []\nchecks:\n  g:\n    files: \"*\"\n    run: \"true\"\n",
        ).unwrap();
        let arch = cfg.architecture.expect("architecture block present");
        assert_eq!(arch.layers[0].name, "data");
        assert!(arch.rules[0].may_import.is_empty());
    }

    #[test]
    fn architecture_defaults_to_none() {
        let cfg: Config = serde_yaml::from_str("checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n").unwrap();
        assert!(cfg.architecture.is_none());
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p ironlint-core arch::config && cargo test -p ironlint-core config::types::tests::parses_config_with_architecture_block`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/ironlint-core/src/arch/config.rs crates/ironlint-core/src/config/types.rs
git commit -m "feat(arch): parse architecture: config block"
```

---

## Task 3: `ImportExtractor` trait + TS/JS extractor

**Files:**
- Create: `crates/ironlint-core/src/arch/extract.rs`
- Create: `crates/ironlint-core/src/arch/languages/typescript.rs`
- Modify: `crates/ironlint-core/src/arch/languages/mod.rs`
- Test: inline in `typescript.rs`

**Interfaces:**
- Consumes: tree-sitter (Task 1).
- Produces: `ImportExtractor` trait, `ImportSource { spec, line }`, `TypescriptExtractor::new()`.

- [ ] **Step 1: Define the trait + `ImportSource`**

`crates/ironlint-core/src/arch/extract.rs`:

```rust
use std::path::Path;

/// A raw import source string as written, plus its 1-indexed line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSource {
    /// As written, pre-resolution: "./components/UserCard", "@/foo", "react".
    pub spec: String,
    /// 1-indexed line of the import statement (for violation messages).
    pub line: usize,
}

/// Extract import source strings from a parsed file. One impl per language.
///
/// The query is near-identical across languages; the per-language work is
/// *which* AST node kinds constitute an import.
pub trait ImportExtractor {
    /// The tree-sitter language for this extractor.
    fn language(&self) -> tree_sitter::Language;

    /// Parse `source` and return import specs in source order.
    fn extract(&self, source: &[u8]) -> Vec<ImportSource> {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&self.language())
            .expect("tree-sitter language loaded");
        let tree = parser.parse(source, None).expect("parse succeeded");
        self.extract_from_tree(&tree, source)
    }

    /// Extract from an already-parsed tree. (Lets the cache reuse a parse.)
    fn extract_from_tree(&self, tree: &tree_sitter::Tree, source: &[u8]) -> Vec<ImportSource>;
}
```

- [ ] **Step 2: Write the failing test for the TS extractor**

`crates/ironlint-core/src/arch/languages/typescript.rs`:

```rust
use crate::arch::extract::{ImportExtractor, ImportSource};

pub struct TypescriptExtractor {
    lang: tree_sitter::Language,
}

impl TypescriptExtractor {
    pub fn new() -> Self {
        Self {
            lang: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
    }
}

impl ImportExtractor for TypescriptExtractor {
    fn language(&self) -> tree_sitter::Language {
        self.lang.clone()
    }

    fn extract_from_tree(
        &self,
        tree: &tree_sitter::Tree,
        source: &[u8],
    ) -> Vec<ImportSource> {
        // Walk the tree; for each import_statement, pull the string-literal
        // source. TS import shapes: `import X from "spec"`, `import {X} from
        // "spec"`, `import "spec"`, `export ... from "spec"` (re-export counts
        // as a dependency edge), dynamic `import("spec")`.
        let mut out = Vec::new();
        let mut cursor = tree_sitter::QueryCursor::new();
        // Use a query that matches the source string of import/export-from.
        let query = tree_sitter::Query::new(
            &self.lang,
            r#"
            (import_statement source: (string) @src)
            (export_statement source: (string) @src)
            (call_expression function: (identifier) @fn arguments: (arguments (string) @src) (#eq? @fn "import"))
            "#,
        )
        .expect("valid query");
        for m in cursor.matches(&query, tree.root_node(), source) {
            for cap in m.captures {
                let node = cap.node;
                let text = node.utf8_text(source).unwrap_or("");
                // Strip quotes: "spec" or 'spec'
                let spec = text.trim_matches(|c| c == '"' || c == '\'').to_string();
                if !spec.is_empty() {
                    out.push(ImportSource {
                        spec,
                        line: node.start_position().row + 1,
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(src: &str) -> Vec<String> {
        TypescriptExtractor::new()
            .extract(src.as_bytes())
            .into_iter()
            .map(|i| i.spec)
            .collect()
    }

    #[test]
    fn extracts_default_import() {
        assert_eq!(
            extract("import Foo from './foo';"),
            vec!["./foo".to_string()]
        );
    }

    #[test]
    fn extracts_named_import() {
        assert_eq!(
            extract("import { a, b } from '@/components/UserCard';"),
            vec!["@/components/UserCard".to_string()]
        );
    }

    #[test]
    fn extracts_side_effect_import() {
        assert_eq!(extract("import './polyfill';"), vec!["./polyfill".to_string()]);
    }

    #[test]
    fn extracts_re_export() {
        assert_eq!(
            extract("export { x } from './bar';"),
            vec!["./bar".to_string()]
        );
    }

    #[test]
    fn extracts_dynamic_import() {
        assert_eq!(
            extract("const m = import('./lazy');"),
            vec!["./lazy".to_string()]
        );
    }

    #[test]
    fn extracts_multiple_in_order() {
        assert_eq!(
            extract("import a from './a';\nimport b from './b';"),
            vec!["./a".to_string(), "./b".to_string()]
        );
    }

    #[test]
    fn external_package_is_still_extracted() {
        // Extraction returns the spec verbatim; resolution (Layer 2) decides
        // whether it's a project file. "react" will fail to resolve and be
        // dropped from the graph — that's correct, not a bug.
        assert_eq!(extract("import React from 'react';"), vec!["react".to_string()]);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p ironlint-core arch::languages::typescript 2>&1 | tail -15`
Expected: FAIL (query syntax or capture issues are common first time). Iterate on the query until tests pass.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ironlint-core arch::languages::typescript`
Expected: all 7 tests PASS.

- [ ] **Step 5: Wire into languages registry stub**

`crates/ironlint-core/src/arch/languages/mod.rs`:

```rust
//! Per-language extractors + resolvers. v1: TypeScript/JavaScript.

pub mod typescript;

use crate::arch::extract::ImportExtractor;
use crate::arch::resolve::Resolver;
use std::path::Path;

/// Returns the (extractor, resolver) for a file, based on extension.
/// None = unsupported language (file dropped from the graph, not an error).
pub fn for_path(path: &Path) -> Option<(Box<dyn ImportExtractor>, Box<dyn Resolver>)> {
    let ext = path.extension()?.to_str()?;
    match ext {
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => {
            let ext: Box<dyn ImportExtractor> = Box::new(typescript::TypescriptExtractor::new());
            let res: Box<dyn Resolver> = Box::new(typescript::TypescriptResolver::new());
            Some((ext, res))
        }
        _ => None,
    }
}
```

(This references `Resolver`/`TypescriptResolver` from Task 4 — that's fine, Task 4 lands next; the stub compiles once Task 4 is in. If you want this task to compile standalone, gate the `for_path` body behind a `todo!()` until Task 4 and replace in Task 4 Step 1.)

- [ ] **Step 6: Commit**

```bash
git add crates/ironlint-core/src/arch/extract.rs \
        crates/ironlint-core/src/arch/languages/
git commit -m "feat(arch): ImportExtractor trait + TS/JS extractor"
```

---

## Task 4: `Resolver` trait + TS/JS resolver

**Files:**
- Modify: `crates/ironlint-core/src/arch/resolve.rs`
- Modify: `crates/ironlint-core/src/arch/languages/typescript.rs`
- Test: inline in `typescript.rs`

**Interfaces:**
- Consumes: `ImportSource` (Task 3).
- Produces: `Resolver` trait, `TypescriptResolver::new()`, `resolve(spec, importer, root) -> Option<PathBuf>`.

- [ ] **Step 1: Define the `Resolver` trait**

`crates/ironlint-core/src/arch/resolve.rs`:

```rust
use std::path::{Path, PathBuf};

/// Resolve a raw import source string to an absolute file path on disk.
///
/// Returns `None` for anything that isn't a project-internal file: external
/// packages, stdlib, bare specifiers with no resolvable target. Unresolved
/// imports are dropped from the graph — they are not architectural edges.
///
/// Resolution is best-effort and conservative: a false drop is acceptable,
/// a false Block is not. The resolver never blocks.
pub trait Resolver: Send + Sync {
    fn resolve(&self, spec: &str, importer: &Path, root: &Path) -> Option<PathBuf>;
}

/// Shared helper: given a base path, try the common TS/JS extensions and
/// `/index.*` barrel forms. Returns the first existing file.
pub fn try_extensions(base: &Path) -> Option<PathBuf> {
    let suffixes: [&str; 16] = [
        "", ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs",
        ".d.ts", "/index.ts", "/index.tsx", "/index.js", "/index.jsx",
        "/index.mjs", "/index.cjs",
    ];
    for suffix in suffixes {
        let candidate = if suffix.is_empty() {
            base.to_path_buf()
        } else if suffix.starts_with('/') {
            base.join(&suffix[1..])
        } else {
            PathBuf::from(format!("{}{}", base.display(), suffix))
        };
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
```

- [ ] **Step 2: Write the failing test for the TS resolver**

Append to `crates/ironlint-core/src/arch/languages/typescript.rs`:

```rust
use crate::arch::resolve::Resolver;
use std::path::{Path, PathBuf};

pub struct TypescriptResolver;

impl TypescriptResolver {
    pub fn new() -> Self {
        Self
    }
}

impl Resolver for TypescriptResolver {
    fn resolve(&self, spec: &str, importer: &Path, root: &Path) -> Option<PathBuf> {
        // Only relative (./ ../) and alias (@/...) specs resolve to project
        // files. Bare specifiers ("react", "lodash") are external → None.
        if spec.starts_with("./") || spec.starts_with("../") {
            let base = importer.parent()?;
            let joined = base.join(spec);
            return crate::arch::resolve::try_extensions(&joined);
        }
        if spec.starts_with('@') {
            // Alias resolution: read tsconfig paths. v1: if no tsconfig found,
            // drop (None). Full alias support is the hard part — see Step 4.
            return self.resolve_alias(spec, root);
        }
        // Bare specifier → external package. Dropped.
        None
    }
}

impl TypescriptResolver {
    fn resolve_alias(&self, _spec: &str, _root: &Path) -> Option<PathBuf> {
        // v1: alias resolution reads tsconfig.json `compilerOptions.paths`.
        // Stubbed to None here; implemented in Task 5. A spec like "@/foo"
        // with no tsconfig → None (dropped, not a block).
        None
    }
}

#[cfg(test)]
mod resolver_tests {
    use super::*;
    use std::fs;

    fn tmp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.ts"), "export const a = 1;").unwrap();
        fs::create_dir(dir.path().join("comp")).unwrap();
        fs::write(dir.path().join("comp").join("index.ts"), "export const c = 1;").unwrap();
        fs::write(dir.path().join("b.tsx"), "export const b = 1;").unwrap();
        dir
    }

    #[test]
    fn resolves_relative_with_extension() {
        let dir = tmp_repo();
        let importer = dir.path().join("a.ts");
        let r = TypescriptResolver::new();
        assert_eq!(
            r.resolve("./b", &importer, dir.path()),
            Some(dir.path().join("b.tsx"))
        );
    }

    #[test]
    fn resolves_barrel_index() {
        let dir = tmp_repo();
        let importer = dir.path().join("a.ts");
        let r = TypescriptResolver::new();
        assert_eq!(
            r.resolve("./comp", &importer, dir.path()),
            Some(dir.path().join("comp").join("index.ts"))
        );
    }

    #[test]
    fn drops_external_package() {
        let dir = tmp_repo();
        let importer = dir.path().join("a.ts");
        let r = TypescriptResolver::new();
        assert_eq!(r.resolve("react", &importer, dir.path()), None);
    }

    #[test]
    fn drops_unresolvable_relative() {
        let dir = tmp_repo();
        let importer = dir.path().join("a.ts");
        let r = TypescriptResolver::new();
        assert_eq!(r.resolve("./nonexistent", &importer, dir.path()), None);
    }
}
```

Add `tempfile = "3"` to `crates/ironlint-core/Cargo.toml` `[dev-dependencies]` if not already present.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p ironlint-core arch::languages::typescript::resolver_tests 2>&1 | tail -15`
Expected: FAIL (resolver not implemented / `try_extensions` buggy).

- [ ] **Step 4: Implement until tests pass**

Fix `try_extensions` to the clean single-loop version. Run:
`cargo test -p ironlint-core arch::languages::typescript::resolver_tests`
Expected: all 4 PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-core/src/arch/resolve.rs \
        crates/ironlint-core/src/arch/languages/typescript.rs \
        crates/ironlint-core/Cargo.toml
git commit -m "feat(arch): Resolver trait + TS/JS resolver (relative + barrel)"
```

---

## Task 5: TS path-alias resolution (`tsconfig` paths)

**Files:**
- Modify: `crates/ironlint-core/src/arch/languages/typescript.rs`
- Test: inline

**Interfaces:**
- Consumes: `Resolver` (Task 4).
- Produces: `TypescriptResolver::resolve_alias` now reads `tsconfig.json`.

- [ ] **Step 1: Write the failing test**

Append to `resolver_tests`:

```rust
    #[test]
    fn resolves_path_alias() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src").join("components")).unwrap();
        fs::write(
            dir.path().join("src").join("components").join("UserCard.tsx"),
            "export const X = 1;",
        ).unwrap();
        fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
        ).unwrap();
        let importer = dir.path().join("src").join("main.ts");
        fs::write(&importer, "").unwrap();
        let r = TypescriptResolver::new();
        assert_eq!(
            r.resolve("@/components/UserCard", &importer, dir.path()),
            Some(dir.path().join("src").join("components").join("UserCard.tsx"))
        );
    }

    #[test]
    fn alias_without_tsconfig_drops() {
        let dir = tempfile::tempdir().unwrap();
        let importer = dir.path().join("a.ts");
        fs::write(&importer, "").unwrap();
        let r = TypescriptResolver::new();
        assert_eq!(r.resolve("@/foo", &importer, dir.path()), None);
    }
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p ironlint-core arch::languages::typescript::resolver_tests::resolves_path_alias`
Expected: FAIL (alias returns None).

- [ ] **Step 3: Implement alias resolution**

Replace the `resolve_alias` stub. Read `tsconfig.json` from `root`, parse `compilerOptions.baseUrl` + `compilerOptions.paths` (a map of alias pattern → list of targets). Match `spec` against each alias pattern (`@/*` → replace `*` with the suffix), join against `baseUrl`, then `try_extensions`.

```rust
    fn resolve_alias(&self, spec: &str, root: &Path) -> Option<PathBuf> {
        let tsconfig = root.join("tsconfig.json");
        let content = fs::read_to_string(&tsconfig).ok()?;
        let v: serde_json::Value = serde_json::from_str(&content).ok()?;
        let co = v.get("compilerOptions")?;
        let base_url = co.get("baseUrl").and_then(|b| b.as_str()).unwrap_or(".");
        let paths = co.get("paths")?.as_object()?;
        for (alias, targets) in paths {
            if let Some(suffix) = match_alias(alias, spec) {
                let target = targets.as_array()?.first()?.as_str()?;
                let resolved = target.replacing("*", &suffix);
                let candidate = root.join(base_url).join(resolved);
                if let Some(found) = crate::arch::resolve::try_extensions(&candidate) {
                    return Some(found);
                }
            }
        }
        None
    }
```

Add `match_alias(alias: &str, spec: &str) -> Option<String>` helper: if alias ends with `/*`, match prefix and return the suffix; else exact match returning empty string. Add `serde_json` to `crates/ironlint-core/Cargo.toml` `[dependencies]` if not present (it likely is, via existing deps — check first).

- [ ] **Step 4: Run tests**

Run: `cargo test -p ironlint-core arch::languages::typescript::resolver_tests`
Expected: all 6 PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-core/src/arch/languages/typescript.rs crates/ironlint-core/Cargo.toml
git commit -m "feat(arch): TS path-alias resolution via tsconfig paths"
```

---

## Task 6: `DepGraph` + layer classification

**Files:**
- Create: `crates/ironlint-core/src/arch/graph.rs`
- Test: inline

**Interfaces:**
- Consumes: `ArchConfig` (Task 2), `ImportExtractor` + `Resolver` (Tasks 3-5).
- Produces: `DepGraph`, `Node`, `Edge`, `LayerId`, `classify(path) -> Option<LayerId>`, `DepGraph::build(root, config)`.

- [ ] **Step 1: Define the types + classification**

`crates/ironlint-core/src/arch/graph.rs`:

```rust
use crate::arch::config::ArchConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Index into `ArchConfig.layers`. None = unlayered.
pub type LayerId = usize;

#[derive(Debug, Clone)]
pub struct Edge {
    pub target: PathBuf,
    pub spec: String,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub layer: Option<LayerId>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Default)]
pub struct DepGraph {
    pub nodes: HashMap<PathBuf, Node>,
    pub root: PathBuf,
}

impl DepGraph {
    /// Classify a file into a layer: first matching layer's globs win
    /// (insertion order). None = unlayered.
    pub fn classify(&self, config: &ArchConfig, path: &Path) -> Option<LayerId> {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        let rel_str = rel.to_string_lossy();
        for (i, layer) in config.layers.iter().enumerate() {
            for glob in &layer.globs {
                if glob_matches(glob, &rel_str) {
                    return Some(i);
                }
            }
        }
        None
    }
}
```

`glob_matches` uses the existing `ironlint-core` glob matcher — check `config/scope.rs` for a reusable function; if the scope matcher works on file paths, reuse it. Otherwise use the `globset` crate (likely already a dep). The spec notes scope matching deliberately diverges from raw globset (bare patterns without `/` match at any depth) — for architecture, use **standard globset semantics** (a glob must match the full relative path), since layer globs like `src/components/**` are path-anchored by intent. Document this divergence from `scope.rs`.

- [ ] **Step 2: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch::config::{ArchConfig, LayerDecl};

    fn cfg() -> ArchConfig {
        ArchConfig {
            layers: vec![
                LayerDecl { name: "presentation".into(), globs: vec!["src/components/**".into()] },
                LayerDecl { name: "data".into(), globs: vec!["src/data/**".into()] },
            ],
            rules: vec![],
            ignore: vec![],
        }
    }

    #[test]
    fn classifies_by_first_match() {
        let g = DepGraph { nodes: HashMap::new(), root: PathBuf::from("/repo") };
        let c = cfg();
        assert_eq!(g.classify(&c, Path::new("/repo/src/components/Foo.tsx")), Some(0));
        assert_eq!(g.classify(&c, Path::new("/repo/src/data/db.ts")), Some(1));
    }

    #[test]
    fn unlayered_when_no_match() {
        let g = DepGraph { nodes: HashMap::new(), root: PathBuf::from("/repo") };
        let c = cfg();
        assert_eq!(g.classify(&c, Path::new("/repo/README.md")), None);
    }
}
```

- [ ] **Step 3: Run to verify fail, then implement `glob_matches`**

Run: `cargo test -p ironlint-core arch::graph`
Implement `glob_matches` using `globset` (add to Cargo.toml if missing). Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/ironlint-core/src/arch/graph.rs crates/ironlint-core/Cargo.toml
git commit -m "feat(arch): DepGraph + layer classification"
```

---

## Task 7: Graph builder (walk + extract + resolve)

**Files:**
- Modify: `crates/ironlint-core/src/arch/graph.rs`
- Test: integration

**Interfaces:**
- Consumes: `languages::for_path` (Task 3), `ArchConfig` (Task 2).
- Produces: `DepGraph::build(root, &config) -> Result<DepGraph>`.

- [ ] **Step 1: Write the failing integration test**

`crates/ironlint-core/tests/arch_graph_build.rs`:

```rust
use ironlint_core::arch::config::ArchConfig;
use ironlint_core::arch::graph::DepGraph;
use std::fs;

#[test]
fn builds_graph_from_ts_repo() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    fs::write(
        root.join("src/components/App.tsx"),
        "import { db } from '../data/db';\n",
    ).unwrap();
    fs::write(root.join("src/data/db.ts"), "export const db = 1;\n").unwrap();

    let config: ArchConfig = serde_yaml::from_str(
        "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\n",
    ).unwrap();
    let graph = DepGraph::build(root, &config).unwrap();
    let app = root.join("src/components/App.tsx");
    let node = graph.nodes.get(&app).expect("App node exists");
    assert_eq!(node.edges.len(), 1);
    assert_eq!(node.edges[0].target, root.join("src/data/db.ts"));
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p ironlint-core --test arch_graph_build`
Expected: FAIL (`build` not implemented).

- [ ] **Step 3: Implement `DepGraph::build`**

Walk `root` recursively (skip `ignore` globs + `.git`/`node_modules`). For each file, `languages::for_path` → if Some, extract imports, resolve each against the graph-in-progress, build `Node` with `layer` + `edges`. Collect into `nodes`.

```rust
impl DepGraph {
    pub fn build(root: &Path, config: &ArchConfig) -> anyhow::Result<DepGraph> {
        let mut graph = DepGraph { nodes: HashMap::new(), root: root.to_path_buf() };
        for entry in walk_files(root, &config.ignore)? {
            let Some((extractor, resolver)) = crate::arch::languages::for_path(&entry) else {
                continue; // unsupported language — not a node
            };
            let source = std::fs::read(&entry)?;
            let imports = extractor.extract(&source);
            let edges: Vec<Edge> = imports
                .into_iter()
                .filter_map(|i| {
                    resolver.resolve(&i.spec, &entry, root).map(|target| Edge {
                        target,
                        spec: i.spec,
                        line: i.line,
                    })
                })
                .collect();
            let layer = graph.classify(config, &entry);
            graph.nodes.insert(entry, Node { layer, edges });
        }
        Ok(graph)
    }
}
```

Add a `walk_files` helper (reuse the pattern from `crates/ironlint-cli/src/commands/sweep.rs:56` `walk_files`, adapted to core + ignore globs).

- [ ] **Step 4: Run tests**

Run: `cargo test -p ironlint-core --test arch_graph_build`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-core/src/arch/graph.rs crates/ironlint-core/tests/arch_graph_build.rs
git commit -m "feat(arch): graph builder (walk + extract + resolve)"
```

---

## Task 8: Policy evaluator (whole-graph)

**Files:**
- Create: `crates/ironlint-core/src/arch/evaluate.rs`
- Test: inline + integration

**Interfaces:**
- Consumes: `DepGraph` (Task 7), `ArchConfig` (Task 2).
- Produces: `Violation`, `evaluate(&graph, &config) -> Vec<Violation>`.

- [ ] **Step 1: Define `Violation` + `evaluate`**

```rust
use crate::arch::config::ArchConfig;
use crate::arch::graph::{DepGraph, LayerId};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Violation {
    pub importer: PathBuf,
    pub target: PathBuf,
    pub importer_layer: LayerId,
    pub target_layer: LayerId,
    pub spec: String,
    pub line: usize,
    pub rule_from: String,
}

/// Evaluate the whole graph: for each edge, if the importer's layer has a rule
/// whose `may_import` excludes the target's layer, it's a violation.
/// Unlayered importers (no rule) → no violation. Unlayered targets → allowed.
pub fn evaluate(graph: &DepGraph, config: &ArchConfig) -> Vec<Violation> {
    let mut out = Vec::new();
    for (importer, node) in &graph.nodes {
        let Some(importer_layer) = node.layer else { continue };
        let layer_name = &config.layers[importer_layer].name;
        let Some(rule) = config.rules.iter().find(|r| r.from == *layer_name) else {
            continue; // no rule for this layer → permissive (may import any)
        };
        for edge in &node.edges {
            let Some(target_node) = graph.nodes.get(&edge.target) else { continue };
            let Some(target_layer) = target_node.layer else { continue };
            let target_name = &config.layers[target_layer].name;
            if !rule.may_import.iter().any(|m| m == target_name) {
                out.push(Violation {
                    importer: importer.clone(),
                    target: edge.target.clone(),
                    importer_layer,
                    target_layer,
                    spec: edge.spec.clone(),
                    line: edge.line,
                    rule_from: layer_name.clone(),
                });
            }
        }
    }
    out
}
```

- [ ] **Step 2: Write the failing test**

`crates/ironlint-core/tests/arch_evaluate.rs`:

```rust
use ironlint_core::arch::config::ArchConfig;
use ironlint_core::arch::evaluate::evaluate;
use ironlint_core::arch::graph::DepGraph;
use std::fs;

#[test]
fn flags_forbidden_edge() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    fs::write(root.join("src/data/db.ts"), "export const db = 1;\n").unwrap();
    fs::write(
        root.join("src/components/App.tsx"),
        "import { db } from '../data/db';\n",
    ).unwrap();
    let config: ArchConfig = serde_yaml::from_str(
        "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\nrules:\n  - from: presentation\n    may_import: []\n",
    ).unwrap();
    let graph = DepGraph::build(root, &config).unwrap();
    let violations = evaluate(&graph, &config);
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].rule_from, "presentation");
}

#[test]
fn allows_permitted_edge() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    fs::write(root.join("src/data/db.ts"), "export const db = 1;\n").unwrap();
    fs::write(
        root.join("src/components/App.tsx"),
        "import { db } from '../data/db';\n",
    ).unwrap();
    let config: ArchConfig = serde_yaml::from_str(
        "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\nrules:\n  - from: presentation\n    may_import: [data]\n",
    ).unwrap();
    let graph = DepGraph::build(root, &config).unwrap();
    let violations = evaluate(&graph, &config);
    assert!(violations.is_empty(), "{violations:?}");
}
```

- [ ] **Step 3: Run to verify fail then pass**

Run: `cargo test -p ironlint-core --test arch_evaluate`
Expected: PASS after implementation (Step 1 is the implementation).

- [ ] **Step 4: Commit**

```bash
git add crates/ironlint-core/src/arch/evaluate.rs crates/ironlint-core/tests/arch_evaluate.rs
git commit -m "feat(arch): policy evaluator (whole-graph)"
```

---

## Task 9: Content-addressed node cache + per-write outgoing evaluation

**Files:**
- Create: `crates/ironlint-core/src/arch/cache.rs`
- Modify: `crates/ironlint-core/src/arch/evaluate.rs` (add `evaluate_outgoing`)
- Test: integration

**Interfaces:**
- Consumes: `DepGraph` (Task 7), `ImportExtractor` + `Resolver` (Tasks 3-5).
- Produces: `NodeCache`, `evaluate_outgoing(proposed_content, proposed_path, &cache, &config) -> Vec<Violation>`.

- [ ] **Step 1: Define the cache**

`crates/ironlint-core/src/arch/cache.rs`:

```rust
use crate::arch::graph::{Edge, LayerId};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CachedNode {
    pub hash: u64,
    pub layer: Option<LayerId>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Default)]
pub struct NodeCache {
    pub entries: HashMap<PathBuf, CachedNode>,
}

impl NodeCache {
    /// Build a cache from a freshly-built graph.
    pub fn from_graph(graph: &crate::arch::graph::DepGraph) -> Self {
        let mut cache = NodeCache::default();
        for (path, node) in &graph.nodes {
            // Hash is computed from on-disk content at build time; stored so
            // we can detect later changes. (Build sets it; see refresh_stale.)
            cache.entries.insert(
                path.clone(),
                CachedNode { hash: 0, layer: node.layer, edges: node.edges.clone() },
            );
        }
        cache
    }

    /// Re-read a node if its on-disk content hash differs from cached.
    /// Returns true if the node was refreshed.
    pub fn refresh_if_stale(&mut self, path: &PathBuf) -> bool {
        let Ok(content) = std::fs::read(path) else {
            self.entries.remove(path);
            return true;
        };
        let hash = twox_hash::xxhash3_64::default().finish(&content);
        if self.entries.get(path).map_or(true, |n| n.hash != hash) {
            // Re-extract + re-resolve this single node. (Done in engine, not
            // here — cache stores the result.) Mark for refresh.
            true
        } else {
            false
        }
    }
}
```

- [ ] **Step 2: Add `evaluate_outgoing`**

In `evaluate.rs`:

```rust
/// Evaluate only the proposed file's outgoing edges (per-write mode).
/// `proposed_content` is the not-yet-written file content; `proposed_path` is
/// its eventual on-disk path. The cache holds the rest of the graph.
///
/// Cannot check incoming edges (the proposed file isn't on disk for others
/// to import). That's the honest per-write split — see spec §graph cache.
pub fn evaluate_outgoing(
    proposed_content: &[u8],
    proposed_path: &Path,
    root: &Path,
    cache: &NodeCache,
    config: &ArchConfig,
) -> anyhow::Result<Vec<Violation>> {
    let Some((extractor, resolver)) = crate::arch::languages::for_path(proposed_path) else {
        return Ok(vec![]); // unsupported language → no violations
    };
    let imports = extractor.extract(proposed_content);
    let graph = DepGraph {
        nodes: cache.entries.iter().map(|(p, n)| (p.clone(), crate::arch::graph::Node { layer: n.layer, edges: n.edges.clone() })).collect(),
        root: root.to_path_buf(),
    };
    let importer_layer = graph.classify(config, proposed_path);
    let Some(importer_layer) = importer_layer else { return Ok(vec![]); };
    let layer_name = &config.layers[importer_layer].name;
    let Some(rule) = config.rules.iter().find(|r| r.from == *layer_name) else {
        return Ok(vec![]); // permissive
    };
    let mut out = Vec::new();
    for imp in imports {
        let Some(target) = resolver.resolve(&imp.spec, proposed_path, root) else { continue };
        let Some(target_node) = graph.nodes.get(&target) else { continue };
        let Some(target_layer) = target_node.layer else { continue };
        let target_name = &config.layers[target_layer].name;
        if !rule.may_import.iter().any(|m| m == target_name) {
            out.push(Violation {
                importer: proposed_path.to_path_buf(),
                target,
                importer_layer,
                target_layer,
                spec: imp.spec,
                line: imp.line,
                rule_from: layer_name.clone(),
            });
        }
    }
    Ok(out)
}
```

- [ ] **Step 3: Write the critical correctness test (multi-file edit)**

`crates/ironlint-core/tests/arch_cache_correctness.rs`:

```rust
// The hard case: write A (adds export), then write B (imports from A).
// The cache must reflect A's new content when evaluating B, or B's import
// resolves to a stale/missing path (false Block or false Pass).
use ironlint_core::arch::*;
use std::fs;

#[test]
fn per_write_sees_prior_write_in_session() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/data")).unwrap();
    fs::create_dir_all(root.join("src/components")).unwrap();
    // A doesn't exist yet on disk.
    let config: config::ArchConfig = serde_yaml::from_str(
        "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\nrules:\n  - from: presentation\n    may_import: [data]\n",
    ).unwrap();
    // Build graph (empty — A not on disk). Cache it.
    let graph = graph::DepGraph::build(root, &config).unwrap();
    let mut cache = cache::NodeCache::from_graph(&graph);
    // Simulate write of A: content lands, cache refreshes A.
    let a_path = root.join("src/data/a.ts");
    let a_content = b"export const a = 1;\n";
    cache.insert_virtual(&a_path, a_content, root, &config); // helper: extract+resolve+cache
    // Now evaluate B's proposed content importing from A.
    let b_path = root.join("src/components/b.tsx");
    let b_content = b"import { a } from '../data/a';\n";
    let violations = evaluate::evaluate_outgoing(b_content, &b_path, root, &cache, &config).unwrap();
    // A is in layer "data"; presentation may_import [data] → permitted → no violation.
    assert!(violations.is_empty(), "per-write must see A's session write: {violations:?}");
}
```

(Add `insert_virtual` to `NodeCache`: extract imports from content, resolve, store node with content hash. This is the mechanism that lets per-write see prior writes in the session.)

- [ ] **Step 4: Run to verify fail then pass**

Run: `cargo test -p ironlint-core --test arch_cache_correctness`
Expected: PASS after implementing `insert_virtual` + wiring `evaluate_outgoing`.

- [ ] **Step 5: Mutation-test the cache**

Run: `cargo mutants --file 'crates/ironlint-core/src/arch/cache.rs'`
Expected: no survivors. A surviving mutant = false pass risk; fix the test.

- [ ] **Step 6: Commit**

```bash
git add crates/ironlint-core/src/arch/cache.rs crates/ironlint-core/src/arch/evaluate.rs \
        crates/ironlint-core/tests/arch_cache_correctness.rs
git commit -m "feat(arch): content-addressed cache + per-write outgoing eval"
```

---

## Task 10: `ArchEngine` — the `check`/`graph`/`why` entry points

**Files:**
- Create: `crates/ironlint-core/src/arch/engine.rs`
- Test: integration

**Interfaces:**
- Consumes: all prior arch modules.
- Produces: `ArchEngine::check(...) -> ArchOutcome`, `graph(...)`, `why(...)`.

- [ ] **Step 1: Define the engine + outcome**

```rust
use crate::arch::config::ArchConfig;
use crate::arch::evaluate::{evaluate, evaluate_outgoing, Violation};
use crate::arch::graph::DepGraph;
use crate::arch::cache::NodeCache;
use std::path::{Path, PathBuf};

pub enum ArchOutcome {
    Pass,
    Block { violations: Vec<Violation> },
    InternalError(String),
}

pub struct ArchEngine;

impl ArchEngine {
    /// Whole-graph check (pre-commit/sweep). Exit 0/2/3.
    pub fn check_whole(root: &Path, config: &ArchConfig) -> ArchOutcome {
        let graph = match DepGraph::build(root, config) {
            Ok(g) => g,
            Err(e) => return ArchOutcome::InternalError(format!("{e:#}")),
        };
        let violations = evaluate(&graph, config);
        if violations.is_empty() { ArchOutcome::Pass } else { ArchOutcome::Block { violations } }
    }

    /// Per-write check (outgoing only). `content` = proposed file content.
    pub fn check_write(root: &Path, config: &ArchConfig, proposed: &Path, content: &[u8]) -> ArchOutcome {
        let graph = match DepGraph::build(root, config) {
            Ok(g) => g,
            Err(e) => return ArchOutcome::InternalError(format!("{e:#}")),
        };
        let mut cache = NodeCache::from_graph(&graph);
        // Refresh any nodes whose on-disk content changed since build (session edits).
        cache.refresh_stale(root);
        match evaluate_outgoing(content, proposed, root, &cache, config) {
            Ok(v) if v.is_empty() => ArchOutcome::Pass,
            Ok(v) => ArchOutcome::Block { violations: v },
            Err(e) => ArchOutcome::InternalError(format!("{e:#}")),
        }
    }

    pub fn graph(root: &Path, config: &ArchConfig) -> Result<DepGraph, String> {
        DepGraph::build(root, config).map_err(|e| format!("{e:#}"))
    }

    pub fn why(root: &Path, config: &ArchConfig, path: &Path) -> Result<Vec<Violation>, String> {
        let g = DepGraph::build(root, config).map_err(|e| format!("{e:#}"))?;
        Ok(evaluate(&g, config).into_iter().filter(|v| v.importer == path).collect())
    }
}
```

- [ ] **Step 2: Test the outcomes**

`crates/ironlint-core/tests/arch_engine.rs` — assert `check_whole` returns `Block` on the forbidden-edge fixture, `Pass` on the permitted one; `check_write` returns `Block` for a proposed file with a forbidden import; `why` returns the violations for one importer.

- [ ] **Step 3: Run + commit**

```bash
cargo test -p ironlint-core --test arch_engine
git add crates/ironlint-core/src/arch/engine.rs crates/ironlint-core/tests/arch_engine.rs
git commit -m "feat(arch): ArchEngine check/graph/why entry points"
```

---

## Task 11: `ironlint arch` CLI subcommand

**Files:**
- Create: `crates/ironlint-cli/src/commands/arch.rs`
- Modify: `crates/ironlint-cli/src/cli.rs` (add `Arch` variant)
- Modify: `crates/ironlint-cli/src/commands/mod.rs`
- Modify: `crates/ironlint-cli/src/main.rs` (dispatch)
- Test: `crates/ironlint-cli/tests/cli_arch.rs`

**Interfaces:**
- Consumes: `ArchEngine` (Task 10).
- Produces: `ironlint arch check|graph|why` with exit codes 0/2/3 (check) and 0/3 (graph/why).

- [ ] **Step 1: Add the clap subcommand**

In `crates/ironlint-cli/src/cli.rs`, add to the `Command` enum:

```rust
    Arch {
        /// Path to the layers YAML file (the `architecture:` block, standalone).
        #[arg(long)]
        layers: Option<PathBuf>,
        /// Project root (default: cwd).
        #[arg(long)]
        root: Option<PathBuf>,
        /// Event: write (per-file, reads stdin) or pre-commit (whole graph).
        #[arg(long)]
        event: Option<String>,
        /// The proposed file path (write mode only).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Emit graph as DOT (graph subcommand) / JSON.
        #[arg(long)]
        json: bool,
        /// Subcommand: check | graph | why <path>
        #[command(subcommand)]
        sub: Option<ArchSub>,
    },
```

(Shape the subcommands `check`/`graph`/`why` per clap's subcommand pattern. If the existing `cli.rs` uses a flat enum rather than nested subcommands, follow whatever pattern `init`/`doctor` use — match the house style.)

- [ ] **Step 2: Implement `commands/arch.rs`**

```rust
use anyhow::Result;
use std::io::Read;
use std::path::PathBuf;

pub fn run(layers: Option<PathBuf>, root: Option<PathBuf>, event: Option<String>, file: Option<PathBuf>, json: bool, sub: Option<crate::cli::ArchSub>) -> Result<i32> {
    let root = root.unwrap_or_else(|| std::env::current_dir().unwrap());
    let layers_path = layers.unwrap_or_else(|| root.join(".ironlint").join("arch.yml"));
    let content = std::fs::read_to_string(&layers_path)
        .map_err(|e| anyhow::anyhow!("reading layers file {}: {e}", layers_path.display()))?;
    let config: ironlint_core::arch::config::ArchConfig = serde_yaml::from_str(&content)?;
    match sub {
        Some(crate::cli::ArchSub::Check) => run_check(&root, &config, event, file),
        Some(crate::cli::ArchSub::Graph) => { run_graph(&root, &config, json); Ok(0) }
        Some(crate::cli::ArchSub::Why { path }) => { run_why(&root, &config, &path); Ok(0) }
        None => Ok(0), // shouldn't happen
    }
}
```

`run_check`: if `event == write` and `file` set, read stdin (proposed content), call `ArchEngine::check_write`, map `Pass→0`, `Block{violations}→2` (print violations), `InternalError→3` (print to stderr). Else `check_whole` → same mapping.

- [ ] **Step 3: Wire dispatch in `main.rs`**

Add `Command::Arch { .. } => commands::arch::run(...)?,` mirroring the existing arms. Add `pub mod arch;` to `commands/mod.rs`.

- [ ] **Step 4: Write e2e tests**

`crates/ironlint-cli/tests/cli_arch.rs`:

```rust
use assert_cmd::Command;

#[test]
fn arch_check_blocks_on_forbidden_edge() {
    let dir = tempfile::tempdir().unwrap();
    // ... write fixture (components/App.tsx imports data/db.ts, rule forbids)
    // ... write .ironlint/arch.yml
    Command::cargo_bin("ironlint").unwrap()
        .args(["arch", "check", "--root", dir.path().to_str().unwrap(), "--layers", layers_path])
        .assert()
        .code(2);
}

#[test]
fn arch_check_passes_when_clean() { /* ... code(0) */ }

#[test]
fn arch_graph_exits_0() { /* ... code(0), stdout contains "digraph" */ }

#[test]
fn arch_why_exits_0() { /* ... code(0) */ }
```

- [ ] **Step 5: Run + commit**

```bash
cargo test -p ironlint-cli --test cli_arch
git add crates/ironlint-cli/src/cli.rs crates/ironlint-cli/src/commands/arch.rs \
        crates/ironlint-cli/src/commands/mod.rs crates/ironlint-cli/src/main.rs \
        crates/ironlint-cli/tests/cli_arch.rs
git commit -m "feat(cli): ironlint arch check|graph|why subcommand"
```

---

## Task 12: Lowering — `architecture:` block → synthetic `__arch__` check

**Files:**
- Create: `crates/ironlint-core/src/arch/lowering.rs`
- Modify: `crates/ironlint-core/src/runner.rs` (call lowering after extends)
- Test: integration

**Interfaces:**
- Consumes: `Config.architecture` (Task 2).
- Produces: `lower_architecture(&mut Config)` inserts a `__arch__` check.

- [ ] **Step 1: Write the failing test**

`crates/ironlint-core/tests/arch_lowering.rs`:

```rust
use ironlint_core::arch::lowering::lower_architecture;
use ironlint_core::config::Config;

#[test]
fn lowers_architecture_to_synthetic_check() {
    let mut cfg: Config = serde_yaml::from_str(
        "architecture:\n  layers:\n    - name: data\n      globs: [\"src/data/**\"]\n  rules:\n    - from: data\n      may_import: []\nchecks:\n  g:\n    files: \"*\"\n    run: \"true\"\n",
    ).unwrap();
    lower_architecture(&mut cfg);
    let arch = cfg.checks.get("__arch__").expect("synthetic __arch__ check inserted");
    assert!(arch.run.as_deref().unwrap().contains("ironlint arch check"));
    assert!(arch.files.iter().any(|f| f == "**/*"));
}
```

- [ ] **Step 2: Implement lowering**

```rust
use crate::config::types::{Check, Config, Lifecycle};

pub fn lower_architecture(cfg: &mut Config) {
    let Some(arch) = cfg.architecture.take() else { return };
    let yaml = serde_yaml::to_string(&arch).unwrap_or_default();
    // The run shells out to `ironlint arch check`. The layers tempfile is
    // materialized by the runner (Task 13) and passed via $IRONLINT_ARCH_LAYERS.
    let run = "ironlint arch check --layers \"$IRONLINT_ARCH_LAYERS\" --root \"$IRONLINT_ROOT\" --event \"$IRONLINT_EVENT\" --file \"$IRONLINT_FILE\"".to_string();
    cfg.checks.insert(
        "__arch__".to_string(),
        Check {
            files: vec!["**/*".to_string()],
            run: Some(run),
            steps: None,
            on: vec![Lifecycle::Write, Lifecycle::PreCommit],
            name: Some("architecture".to_string()),
        },
    );
    // Stash the serialized layers for the runner to materialize.
    cfg.arch_layers_yaml = Some(yaml);
}
```

Add `pub arch_layers_yaml: Option<String>` to `Config` (transient — not serialized; `#[serde(skip)]`).

- [ ] **Step 3: Call lowering in the runner**

In `crates/ironlint-core/src/runner.rs`, after `extends::resolve` (in `IronLintEngine::load`), call `arch::lowering::lower_architecture(&mut cfg)`.

- [ ] **Step 4: Run + commit**

```bash
cargo test -p ironlint-core --test arch_lowering
git add crates/ironlint-core/src/arch/lowering.rs crates/ironlint-core/src/config/types.rs \
        crates/ironlint-core/src/runner.rs crates/ironlint-core/tests/arch_lowering.rs
git commit -m "feat(arch): lower architecture: block to synthetic __arch__ check"
```

---

## Task 13: Materialize `$IRONLINT_ARCH_LAYERS` tempfile

**Files:**
- Modify: `crates/ironlint-core/src/runner.rs`
- Modify: `crates/ironlint-core/src/engine/gate.rs` (export the env var)
- Test: integration

**Interfaces:**
- Consumes: `Config.arch_layers_yaml` (Task 12), `TmpFileGuard` pattern (`runner.rs`).
- Produces: `$IRONLINT_ARCH_LAYERS` set in the check env when `__arch__` runs.

- [ ] **Step 1: Write the failing test**

`crates/ironlint-core/tests/arch_layers_env.rs` — a check whose `run` echoes `$IRONLINT_ARCH_LAYERS` and asserts it points to a file containing the serialized layers YAML. (Mirror the existing `tmpfile_materialized_with_content_ext_and_cleaned` test in the runner test suite.)

- [ ] **Step 2: Implement materialization**

In the runner, when dispatching the `__arch__` check (or any check whose `run` references `$IRONLINT_ARCH_LAYERS`), materialize a tempfile from `cfg.arch_layers_yaml` using the `materialize_tmpfile` + `TmpFileGuard` pattern, set `$IRONLINT_ARCH_LAYERS` to its path, and clean up after. Reuse the existing `check_references_tmpfile` pattern: add `check_references_arch_layers(check)` that scans the `run` for `$IRONLINT_ARCH_LAYERS`.

- [ ] **Step 3: Wire into `GateEnv` / `build_check_env`**

Add `arch_layers: Option<&Path>` to `GateEnv` (`engine/gate.rs`), export as `$IRONLINT_ARCH_LAYERS` in `build_check_env` (mirroring `tmpfile`).

- [ ] **Step 4: Run + commit**

```bash
cargo test -p ironlint-core --test arch_layers_env
git add crates/ironlint-core/src/runner.rs crates/ironlint-core/src/engine/gate.rs \
        crates/ironlint-core/tests/arch_layers_env.rs
git commit -m "feat(arch): materialize $IRONLINT_ARCH_LAYERS tempfile"
```

---

## Task 14: `extends` whole-block merge for `architecture`

**Files:**
- Modify: `crates/ironlint-core/src/config/extends.rs` (`merge_inherited`)
- Test: `crates/ironlint-core/tests/arch_extends.rs`

**Interfaces:**
- Consumes: `Config.architecture` (Task 2).
- Produces: `merge_inherited` handles `architecture` (local wins, whole-block).

- [ ] **Step 1: Write the failing test**

`crates/ironlint-core/tests/arch_extends.rs` — base config with `architecture:`, child without → child inherits; child with its own → child's replaces base's.

- [ ] **Step 2: Add the merge line**

In `merge_inherited` (`extends.rs`), after the `execution` line:

```rust
    local.architecture = local.architecture.take().or(inherited.architecture);
```

(Whole-block: if local sets `architecture`, it wins entirely; else inherit. Matches the spec's "whole-block replace" decision.)

- [ ] **Step 3: Run + commit**

```bash
cargo test -p ironlint-core --test arch_extends
git add crates/ironlint-core/src/config/extends.rs crates/ironlint-core/tests/arch_extends.rs
git commit -m "feat(arch): extends whole-block merge for architecture:"
```

---

## Task 15: Trust integration + `doctor` grammar check

**Files:**
- Modify: `crates/ironlint-cli/src/commands/doctor.rs` (add `check_arch_grammars`)
- Test: `crates/ironlint-cli/tests/cli_e2e_doctor.rs`

**Interfaces:**
- Consumes: `languages::for_path` (Task 3) — to detect which grammars a repo needs.
- Produces: doctor reports a warn row if a repo file's language has no loaded grammar.

- [ ] **Step 1: Write the failing test**

In `cli_e2e_doctor.rs` — a repo with a `.rs` file (no Rust resolver in v1) → doctor warns "architecture: Rust not supported in v1 (file dropped from graph)".

- [ ] **Step 2: Implement `check_arch_grammars`**

Walk the repo, collect extensions, for each that `languages::for_path` returns `None` for (and that isn't a known-non-code file), emit a warn row. Only runs if `architecture:` block is present (else skip — no arch enforcement active).

- [ ] **Step 3: Verify trust is free**

Confirm (no code needed): editing the `architecture:` block changes `compute_hash` (it's config bytes). Add a test in `trust.rs` mirroring `editing_config_changes_hash` but editing a layer glob — assert hash changes.

- [ ] **Step 4: Run + commit**

```bash
cargo test -p ironlint-cli --test cli_e2e_doctor
cargo test -p ironlint-core editing_architecture 2>/dev/null || cargo test -p ironlint-core trust
git add crates/ironlint-cli/src/commands/doctor.rs crates/ironlint-cli/tests/cli_e2e_doctor.rs \
        crates/ironlint-core/src/trust.rs
git commit -m "feat(arch): doctor grammar check + trust covers architecture block"
```

---

## Task 16: End-to-end through `ironlint check` + docs

**Files:**
- Test: `crates/ironlint-cli/tests/cli_e2e_arch_check.rs`
- Modify: `CLAUDE.md` (AGENTS.md) — document the `architecture:` block + `ironlint arch`.

**Interfaces:**
- Consumes: all prior tasks.

- [ ] **Step 1: Write the e2e test**

A repo with `.ironlint.yml` containing an `architecture:` block + a `checks:` entry. Run `ironlint check --file <proposed> --content <bad-import>` → exit 2 with a violation message naming the forbidden edge. Run with a clean file → exit 0. Run `ironlint check` (bare sweep) → exit 2 if the on-disk repo has a violation.

- [ ] **Step 2: Update AGENTS.md**

Add to the "What this is" section: architecture enforcement as a capability. Add the `architecture:` block to the config examples. Note `ironlint arch` as a subcommand. Cross-link the spec.

- [ ] **Step 3: Full test suite + coverage gate + clippy**

```bash
cargo test
cargo clippy --all-targets -- -D warnings
bash scripts/ci-coverage.sh
```
Expected: all green. Fix any file under 80% region coverage. Clean up artifacts: `rm -rf target/llvm-cov-target target/llvm-cov` (per the ci-coverage-cleanup memory).

- [ ] **Step 4: Commit**

```bash
git add crates/ironlint-cli/tests/cli_e2e_arch_check.rs CLAUDE.md
git commit -m "feat(arch): e2e through ironlint check + docs"
```

---

## Self-Review (completed)

**1. Spec coverage:**
- Config shape (layers/rules/ignore) → Task 2. ✓
- Lowering to `__arch__` → Task 12. ✓
- `ironlint arch check|graph|why` → Task 11. ✓
- tree-sitter extraction (TS/JS) → Tasks 3, 5. ✓
- Per-language resolver (TS/JS, incl. aliases) → Tasks 4, 5. ✓
- DepGraph + classification → Task 6. ✓
- Graph builder → Task 7. ✓
- Policy evaluator (whole-graph) → Task 8. ✓
- Content-addressed cache + per-write outgoing → Task 9. ✓
- ArchEngine entry points → Task 10. ✓
- `$IRONLINT_ARCH_LAYERS` materialization → Task 13. ✓
- `extends` whole-block merge → Task 14. ✓
- Trust (free via config hash) → Task 15. ✓
- doctor grammar check → Task 15. ✓
- Exit contract (0/2/3 check; 0/3 graph/why) → Task 11. ✓
- Per-write honest split (outgoing only) → Task 9. ✓
- Out of scope (Rust/Py/Go/PHP, --fix, on-disk cache, additive extends merge, content-aware name rules) → explicitly deferred. ✓

**2. Placeholder scan:** No TBD/TODO in task steps. Some steps say "mirror the existing X pattern" with a file pointer — that's a concrete reference, not a placeholder. The `try_extensions` cleanup note in Task 4 is flagged for the implementer to resolve.

**3. Type consistency:** `ArchConfig`/`LayerDecl`/`RuleDecl` (Task 2) used consistently in Tasks 6-10. `ImportExtractor`/`ImportSource` (Task 3) used in 7, 9. `Resolver` (Task 4) used in 7, 9. `DepGraph`/`Node`/`Edge`/`LayerId` (Task 6) used in 7-10. `Violation` (Task 8) used in 9-11. `ArchOutcome`/`ArchEngine` (Task 10) used in 11. `lower_architecture` (Task 12) called in runner (Task 13). `arch_layers_yaml` field (Task 12) consumed in Task 13. Consistent.

**Gaps fixed inline:** Task 3 Step 5 references `Resolver`/`TypescriptResolver` before Task 4 lands — added a note to gate `for_path` behind `todo!()` until Task 4. Task 6 `glob_matches` needs `globset` dep — flagged. Task 9 `insert_virtual` helper referenced in test but defined in step — confirmed it's added in Step 4.
