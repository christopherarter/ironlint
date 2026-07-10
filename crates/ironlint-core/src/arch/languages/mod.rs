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
