//! Architecture enforcement: language-agnostic layer/dependency-direction rules.
//!
//! See `docs/superpowers/specs/2026-07-09-architecture-enforcement-design.md`.
//! Three layers: extract (tree-sitter) → resolve (per-language) → evaluate
//! (language-agnostic). Exposed via the `ironlint arch` subcommand and the
//! `architecture:` config block (which lowers to a synthetic `__arch__` check).

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
        assert!(!tree.root_node().has_error());
    }
}
