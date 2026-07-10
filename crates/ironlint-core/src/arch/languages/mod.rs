//! Per-language extractors + resolvers. v1: TypeScript/JavaScript.

pub mod typescript;

use crate::arch::extract::ImportExtractor;
use crate::arch::resolve::Resolver;
use std::path::Path;

pub fn for_path(path: &Path) -> Option<(Box<dyn ImportExtractor>, Box<dyn Resolver>)> {
    match path.extension()?.to_str()? {
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => Some((
            Box::new(typescript::TypescriptExtractor::new()),
            Box::new(typescript::TypescriptResolver::new()),
        )),
        _ => None,
    }
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
}
