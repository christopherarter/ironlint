//! Per-language extractors + resolvers. v1: TypeScript/JavaScript.

pub mod typescript;

use crate::arch::extract::ImportExtractor;
use crate::arch::resolve::Resolver;
use std::path::Path;

pub fn for_path(path: &Path) -> Option<(Box<dyn ImportExtractor>, Box<dyn Resolver>)> {
    let lang: tree_sitter::Language = match path.extension()?.to_str()? {
        "ts" | "mts" | "cts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "js" | "jsx" | "mjs" | "cjs" => tree_sitter_javascript::LANGUAGE.into(),
        _ => return None,
    };
    Some((
        Box::new(typescript::TypescriptExtractor::with_language(lang)),
        Box::new(typescript::TypescriptResolver::new()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_path_no_extension() {
        assert!(for_path(Path::new("foo")).is_none());
    }

    #[test]
    fn for_path_unsupported_extension() {
        assert!(for_path(Path::new("foo.rs")).is_none());
    }

    #[test]
    fn tsx_extractor_finds_import_after_jsx() {
        // Bug 3: .tsx must be parsed with the TSX grammar. Plain TypeScript
        // chokes on JSX and can drop the subsequent import.
        let src = "const el = <div/>;\nimport { db } from '../data/db';\n";
        let (extractor, _) = for_path(Path::new("src/Foo.tsx")).unwrap();
        let specs: Vec<_> = extractor
            .extract(src.as_bytes())
            .into_iter()
            .map(|i| i.spec)
            .collect();
        assert_eq!(specs, vec!["../data/db"]);
    }

    #[test]
    fn jsx_extractor_finds_import_after_jsx() {
        // Bug 3: .jsx must be parsed with the JavaScript grammar (which handles
        // JSX), not the TypeScript grammar.
        let src = "const el = <div/>;\nimport { db } from '../data/db';\n";
        let (extractor, _) = for_path(Path::new("src/Foo.jsx")).unwrap();
        let specs: Vec<_> = extractor
            .extract(src.as_bytes())
            .into_iter()
            .map(|i| i.spec)
            .collect();
        assert_eq!(specs, vec!["../data/db"]);
    }

    #[test]
    fn for_path_selects_grammar_by_extension() {
        // Every supported extension must route to a grammar that can parse a
        // plain import. JSX-capable extensions must also handle JSX before an
        // import without dropping it.
        let with_jsx = "const el = <div/>;\nimport { db } from '../data/db';\n";
        let plain = "import { db } from '../data/db';\n";
        let cases: &[(&str, &str, &[&str])] = &[
            ("src/Foo.ts", plain, &["../data/db"]),
            ("src/Foo.mts", plain, &["../data/db"]),
            ("src/Foo.cts", plain, &["../data/db"]),
            ("src/Foo.tsx", with_jsx, &["../data/db"]),
            ("src/Foo.js", plain, &["../data/db"]),
            ("src/Foo.mjs", plain, &["../data/db"]),
            ("src/Foo.cjs", plain, &["../data/db"]),
            ("src/Foo.jsx", with_jsx, &["../data/db"]),
        ];
        for (path, source, expected) in cases {
            let (extractor, _) = for_path(Path::new(path)).unwrap();
            let specs: Vec<_> = extractor
                .extract(source.as_bytes())
                .into_iter()
                .map(|i| i.spec)
                .collect();
            assert_eq!(&specs.as_slice(), expected, "wrong imports for {path}");
        }
    }
}
