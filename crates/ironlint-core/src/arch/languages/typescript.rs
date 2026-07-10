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
        Self {
            lang: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
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
            ",
        )
        .expect("valid query");
        for m in cursor.matches(&query, tree.root_node(), source) {
            for cap in m.captures {
                let node = cap.node;
                let text = node.utf8_text(source).unwrap_or("");
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
        let content = fs::read_to_string(&tsconfig).ok()?;
        let v: serde_json::Value = serde_json::from_str(&content).ok()?;
        let co = v.get("compilerOptions")?;
        let base_url = co.get("baseUrl").and_then(|b| b.as_str()).unwrap_or(".");
        let paths = co.get("paths")?.as_object()?;
        for (alias, targets) in paths {
            if let Some(suffix) = match_alias(alias, spec) {
                let target = targets.as_array()?.first()?.as_str()?;
                let resolved = target.replace('*', &suffix);
                let candidate = root.join(base_url).join(resolved);
                if let Some(found) = crate::arch::resolve::try_extensions(&candidate) {
                    return Some(found);
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
    fn alias_without_tsconfig_drops() {
        let dir = tempfile::tempdir().unwrap();
        let importer = dir.path().join("a.ts");
        fs::write(&importer, "").unwrap();
        let r = TypescriptResolver::new();
        assert_eq!(r.resolve("@/foo", &importer, dir.path()), None);
    }
}
