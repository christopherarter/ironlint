//! TypeScript/JavaScript import extractor.

use crate::arch::extract::{ImportExtractor, ImportSource};
use crate::arch::resolve::Resolver;
use std::fs;
use std::path::{Path, PathBuf};

pub struct TypescriptExtractor {
    lang: tree_sitter::Language,
}

impl TypescriptExtractor {
    pub fn new() -> Self {
        Self::with_language(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
    }

    pub fn with_language(lang: tree_sitter::Language) -> Self {
        Self { lang }
    }
}

impl Default for TypescriptExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl ImportExtractor for TypescriptExtractor {
    fn language(&self) -> tree_sitter::Language {
        self.lang.clone()
    }

    fn extract_from_tree(&self, tree: &tree_sitter::Tree, source: &[u8]) -> Vec<ImportSource> {
        let mut out = Vec::new();
        let mut cursor = tree_sitter::QueryCursor::new();
        let query = tree_sitter::Query::new(
            &self.lang,
            r"
            (import_statement source: (string) @src)
            (export_statement source: (string) @src)
            (call_expression
              function: (import)
              arguments: (arguments (string) @src))
            (call_expression
              function: (identifier) @func
              arguments: (arguments (string) @src))
            ",
        )
        .expect("valid query");
        let src_idx = query.capture_index_for_name("src").expect("src capture");
        let func_idx = query.capture_index_for_name("func").expect("func capture");
        for m in cursor.matches(&query, tree.root_node(), source) {
            // For the `require(...)` pattern, @func and @src are both present
            // in the same match; gate the @src on @func being exactly "require".
            // The other patterns only capture @src, so func is None and the
            // src is emitted unconditionally.
            let mut func_text: Option<&str> = None;
            let mut src_node: Option<tree_sitter::Node> = None;
            for cap in m.captures {
                if cap.index == func_idx {
                    func_text = Some(cap.node.utf8_text(source).unwrap_or(""));
                } else if cap.index == src_idx {
                    src_node = Some(cap.node);
                }
            }
            let Some(node) = src_node else { continue };
            if let Some(func) = func_text {
                if func != "require" {
                    continue;
                }
            }
            let text = node.utf8_text(source).unwrap_or("");
            let spec = text.trim_matches(|c| c == '"' || c == '\'').to_string();
            if !spec.is_empty() {
                out.push(ImportSource {
                    spec,
                    line: node.start_position().row + 1,
                });
            }
        }
        out
    }
}

pub struct TypescriptResolver;

impl TypescriptResolver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TypescriptResolver {
    fn default() -> Self {
        Self::new()
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
            return Self::resolve_alias(spec, root);
        }
        // Bare specifier → external package. Dropped.
        None
    }
}

impl TypescriptResolver {
    fn resolve_alias(spec: &str, root: &Path) -> Option<PathBuf> {
        let tsconfig = root.join("tsconfig.json");
        let jsconfig = root.join("jsconfig.json");
        let content = fs::read_to_string(&tsconfig)
            .or_else(|_| fs::read_to_string(&jsconfig))
            .ok()?;
        let v: serde_json::Value = serde_json::from_str(&content).ok()?;
        let co = v.get("compilerOptions")?;
        let base_url = co.get("baseUrl").and_then(|b| b.as_str()).unwrap_or(".");
        let paths = co.get("paths")?.as_object()?;

        // Collect every alias pattern that matches the spec. TypeScript paths
        // resolution picks the longest matching pattern, not insertion order.
        let mut matches: Vec<(&String, &serde_json::Value, String, usize)> = Vec::new();
        for (alias, targets) in paths {
            if let Some(suffix) = match_alias(alias, spec) {
                let consumed = spec.len() - suffix.len();
                matches.push((alias, targets, suffix, consumed));
            }
        }
        matches.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| b.0.len().cmp(&a.0.len())));

        for (_, targets, suffix, _) in matches {
            let targets = targets.as_array()?;
            for target in targets {
                if let Some(target) = target.as_str() {
                    let resolved = target.replace('*', &suffix);
                    let candidate = root.join(base_url).join(resolved);
                    if let Some(found) = crate::arch::resolve::try_extensions(&candidate) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }
}

fn match_alias(alias: &str, spec: &str) -> Option<String> {
    if let Some(prefix) = alias.strip_suffix("/*") {
        let marker = format!("{}/", prefix);
        spec.strip_prefix(&marker).map(|s| s.to_string())
    } else if alias == spec {
        Some(String::new())
    } else {
        None
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
        assert_eq!(
            extract("import './polyfill';"),
            vec!["./polyfill".to_string()]
        );
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
    fn extracts_commonjs_require() {
        // require("../data/db") is a CommonJS import — must be extracted so a
        // forbidden .cjs import cannot sneak through. (Bug 2.)
        assert_eq!(
            extract("const db = require('../data/db');"),
            vec!["../data/db".to_string()]
        );
    }

    #[test]
    fn extracts_require_with_double_quotes() {
        assert_eq!(
            extract("const x = require(\"./x\");"),
            vec!["./x".to_string()]
        );
    }

    #[test]
    fn extracts_require_alongside_esm() {
        assert_eq!(
            extract("import a from './a';\nconst b = require('./b');"),
            vec!["./a".to_string(), "./b".to_string()]
        );
    }

    #[test]
    fn does_not_extract_arbitrary_identifier_call() {
        // A non-require identifier call with a string arg must NOT be treated
        // as an import — only `require` qualifies. Guards the @func filter.
        assert_eq!(extract("const x = load('./config');"), Vec::<String>::new());
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
        assert_eq!(
            extract("import React from 'react';"),
            vec!["react".to_string()]
        );
    }

    #[test]
    fn records_one_indexed_line_number() {
        let src = "const a = 1;\nimport { x } from './foo';\n";
        let imports = TypescriptExtractor::new().extract(src.as_bytes());
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].line, 2, "import is on line 2 (1-indexed)");
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
        fs::write(
            dir.path().join("comp").join("index.ts"),
            "export const c = 1;",
        )
        .unwrap();
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

    #[test]
    fn resolves_path_alias() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src").join("components")).unwrap();
        fs::write(
            dir.path()
                .join("src")
                .join("components")
                .join("UserCard.tsx"),
            "export const X = 1;",
        )
        .unwrap();
        fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
        )
        .unwrap();
        let importer = dir.path().join("src").join("main.ts");
        fs::write(&importer, "").unwrap();
        let r = TypescriptResolver::new();
        assert_eq!(
            r.resolve("@/components/UserCard", &importer, dir.path()),
            Some(
                dir.path()
                    .join("src")
                    .join("components")
                    .join("UserCard.tsx")
            )
        );
    }

    #[test]
    fn alias_longest_match_wins() {
        // TypeScript paths resolution selects the LONGEST matching pattern,
        // not the first one in object order. (Bug 4.)
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src").join("data")).unwrap();
        fs::create_dir_all(dir.path().join("src").join("legacy").join("data")).unwrap();
        fs::write(
            dir.path().join("src").join("data").join("db.ts"),
            "export const db = 1;",
        )
        .unwrap();
        fs::write(
            dir.path()
                .join("src")
                .join("legacy")
                .join("data")
                .join("db.ts"),
            "export const legacy = 1;",
        )
        .unwrap();
        fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/legacy/*"],"@/data/*":["src/data/*"]}}}"#,
        )
        .unwrap();
        let importer = dir.path().join("src").join("main.ts");
        fs::write(&importer, "").unwrap();
        let r = TypescriptResolver::new();
        assert_eq!(
            r.resolve("@/data/db", &importer, dir.path()),
            Some(dir.path().join("src").join("data").join("db.ts"))
        );
    }

    #[test]
    fn alias_tries_all_targets() {
        // TypeScript tries each entry in the target array until one resolves.
        // (Bug 4.)
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src").join("fallback")).unwrap();
        fs::write(
            dir.path().join("src").join("fallback").join("foo.ts"),
            "export const foo = 1;",
        )
        .unwrap();
        fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/missing/*","src/fallback/*"]}}}"#,
        )
        .unwrap();
        let importer = dir.path().join("src").join("main.ts");
        fs::write(&importer, "").unwrap();
        let r = TypescriptResolver::new();
        assert_eq!(
            r.resolve("@/foo", &importer, dir.path()),
            Some(dir.path().join("src").join("fallback").join("foo.ts"))
        );
    }

    #[test]
    fn alias_falls_back_to_jsconfig() {
        // The resolver is selected for .jsx/.js too, so jsconfig.json should be
        // honored when tsconfig.json is absent.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src").join("components")).unwrap();
        fs::write(
            dir.path().join("src").join("components").join("Button.jsx"),
            "export const Button = 1;",
        )
        .unwrap();
        fs::write(
            dir.path().join("jsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
        )
        .unwrap();
        let importer = dir.path().join("src").join("main.js");
        fs::write(&importer, "").unwrap();
        let r = TypescriptResolver::new();
        assert_eq!(
            r.resolve("@/components/Button", &importer, dir.path()),
            Some(dir.path().join("src").join("components").join("Button.jsx"))
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

    #[test]
    fn bare_alias_exact_match_resolves() {
        // A non-wildcard (bare) alias — `"@/utils": ["src/utils.ts"]`, no `/*` —
        // must resolve its exact spec. Guards the `else if alias == spec` arm
        // in `match_alias` (mutant: `==` → `!=`).
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src").join("utils.ts"),
            "export const u = 1;",
        )
        .unwrap();
        fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/utils":["src/utils.ts"]}}}"#,
        )
        .unwrap();
        let importer = dir.path().join("src").join("main.ts");
        fs::write(&importer, "").unwrap();
        let r = TypescriptResolver::new();
        assert_eq!(
            r.resolve("@/utils", &importer, dir.path()),
            Some(dir.path().join("src").join("utils.ts"))
        );
    }

    #[test]
    fn bare_alias_does_not_match_other_specs() {
        // A bare alias matches ONLY its exact spec — not every spec. This is
        // the assertion that kills the `==` → `!=` mutant: under `!=`, the
        // bare alias would match any `@/...` spec and resolve to its target.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src").join("utils.ts"),
            "export const u = 1;",
        )
        .unwrap();
        fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/utils":["src/utils.ts"]}}}"#,
        )
        .unwrap();
        let importer = dir.path().join("src").join("main.ts");
        fs::write(&importer, "").unwrap();
        let r = TypescriptResolver::new();
        assert_eq!(
            r.resolve("@/something-else", &importer, dir.path()),
            None,
            "bare alias must not match specs other than its exact target"
        );
    }

    #[test]
    fn bare_alias_beats_wildcard_for_exact_spec() {
        // A bare alias (`"@/utils"`, no `/*`) and a wildcard (`"@/*"`) can
        // both match the exact spec `"@/utils"`. Longest-match resolution
        // must pick the bare alias — its marker consumes the whole spec —
        // not the wildcard. (Bug 4 sort key; mutant 1a.)
        //
        // The two targets are deliberately *different files* so the assertion
        // genuinely discriminates: the bare alias points at `src/exact-utils.ts`,
        // while the wildcard `src/*` over `@/utils` resolves to
        // `src/utils/index.ts`. If the wildcard wrongly won the sort, the
        // resolver would return the wildcard's file — a different path than
        // asserted. (The bare-alias `+`/`*` arithmetic mutants at this line
        // are covered by `alias_longest_match_wins`; this one pins the `/`
        // mutant via its divide-by-zero on the bare alias's empty suffix.)
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src").join("utils")).unwrap();
        // Bare-alias target — the CORRECT resolution.
        fs::write(
            dir.path().join("src").join("exact-utils.ts"),
            "export const exact = 1;",
        )
        .unwrap();
        // Wildcard target — what a wrong (wildcard-wins) resolution returns.
        fs::write(
            dir.path().join("src").join("utils").join("index.ts"),
            "export const wildcard = 1;",
        )
        .unwrap();
        // Object order puts the wildcard FIRST so that only longest-match
        // selection picks the bare alias — first-match would not.
        fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"],"@/utils":["src/exact-utils.ts"]}}}"#,
        )
        .unwrap();
        let importer = dir.path().join("src").join("main.ts");
        fs::write(&importer, "").unwrap();
        let r = TypescriptResolver::new();
        assert_eq!(
            r.resolve("@/utils", &importer, dir.path()),
            Some(dir.path().join("src").join("exact-utils.ts")),
            "exact bare alias must win over the broader wildcard"
        );
    }
}
