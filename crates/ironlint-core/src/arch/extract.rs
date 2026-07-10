//! Import extraction: language-agnostic trait and per-language implementations.

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
pub trait ImportExtractor: Send + Sync {
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

    /// Extract from an already-parsed tree. (Lets a caller reuse a parse.)
    fn extract_from_tree(&self, tree: &tree_sitter::Tree, source: &[u8]) -> Vec<ImportSource>;
}
