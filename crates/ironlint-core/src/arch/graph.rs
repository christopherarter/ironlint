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
use std::fs;
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
    /// Build a dependency graph by walking `root`, extracting imports from every
    /// supported source file, resolving them to absolute targets, and classifying
    /// each file into its architecture layer.
    pub fn build(root: &Path, config: &ArchConfig) -> anyhow::Result<Self> {
        let mut graph = Self {
            nodes: HashMap::new(),
            root: root.to_path_buf(),
        };
        for entry in walk_files(root, &config.ignore)? {
            let Some((extractor, resolver)) = crate::arch::languages::for_path(&entry) else {
                continue; // unsupported language — not a node
            };
            let source = fs::read(&entry)?;
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

/// Recursively walk `root`, returning absolute paths to every regular file.
///
/// Skips `.git` and `node_modules` directories entirely, plus any file whose
/// relative path matches one of the `ignore` globs (standard `globset`
/// semantics). Results are sorted for deterministic output.
fn walk_files(root: &Path, ignore: &[String]) -> anyhow::Result<Vec<PathBuf>> {
    let mut builder = GlobSetBuilder::new();
    for glob in ignore {
        builder.add(Glob::new(glob)?);
    }
    let ignore_set = builder.build()?;

    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name();
            if name == ".git" || name == "node_modules" {
                continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap_or(&path);
            if ignore_set.is_match(rel) {
                continue;
            }
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
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
