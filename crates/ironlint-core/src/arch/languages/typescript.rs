//! TypeScript/JavaScript import extractor.

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
}
