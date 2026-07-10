//! Dependency graph for architecture enforcement.
//!
//! A `DepGraph` maps every source file under `root` to a `Node` containing its
//! architecture layer and the imports it declares. Layer classification uses
//! **standard `globset` semantics**: a glob must match the full relative path.
//! This is intentionally stricter than `config::scope`, which auto-prefixes bare
//! patterns with `**/` to emulate bully's file-matching behavior; architecture
//! layer globs such as `src/components/**` are path-anchored by design.

use crate::arch::config::ArchConfig;
use globset::{Glob, GlobSetBuilder};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Index into `ArchConfig.layers`. `None` = unlayered.
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

impl Node {
    pub fn new(layer: Option<LayerId>) -> Self {
        Self {
            layer,
            edges: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
pub struct DepGraph {
    pub nodes: HashMap<PathBuf, Node>,
    pub root: PathBuf,
}

impl DepGraph {
    /// Build an empty dependency graph rooted at `root`.
    ///
    /// Edges are populated by later pipeline stages; classification is performed
    /// on demand using the supplied `ArchConfig`.
    pub fn build(root: impl Into<PathBuf>, _config: &ArchConfig) -> Self {
        Self {
            nodes: HashMap::new(),
            root: root.into(),
        }
    }

    /// Classify a file into a layer: first matching layer's globs win
    /// (insertion order). `None` = unlayered.
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

/// Match a single glob against a path using standard `globset` semantics.
///
/// This deliberately does **not** reuse `config::scope`: architecture layer
/// globs are expected to match the full relative path (`src/components/**`),
/// whereas check file globs in `config::scope` treat bare patterns as
/// `**/<pattern>` for bully compatibility.
fn glob_matches(glob: &str, path: &str) -> bool {
    let Ok(g) = Glob::new(glob) else {
        return false;
    };
    let set = GlobSetBuilder::new().add(g).build();
    match set {
        Ok(set) => set.is_match(path),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch::config::LayerDecl;

    fn cfg() -> ArchConfig {
        ArchConfig {
            layers: vec![
                LayerDecl {
                    name: "presentation".into(),
                    globs: vec!["src/components/**".into()],
                },
                LayerDecl {
                    name: "data".into(),
                    globs: vec!["src/data/**".into()],
                },
            ],
            rules: vec![],
            ignore: vec![],
        }
    }

    #[test]
    fn classifies_by_first_match() {
        let g = DepGraph {
            nodes: HashMap::new(),
            root: PathBuf::from("/repo"),
        };
        let c = cfg();
        assert_eq!(
            g.classify(&c, Path::new("/repo/src/components/Foo.tsx")),
            Some(0)
        );
        assert_eq!(g.classify(&c, Path::new("/repo/src/data/db.ts")), Some(1));
    }

    #[test]
    fn unlayered_when_no_match() {
        let g = DepGraph {
            nodes: HashMap::new(),
            root: PathBuf::from("/repo"),
        };
        let c = cfg();
        assert_eq!(g.classify(&c, Path::new("/repo/README.md")), None);
    }
}
