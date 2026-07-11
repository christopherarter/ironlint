//! Dependency graph for architecture enforcement.
//!
//! A `DepGraph` maps every source file under `root` to a `Node` containing its
//! architecture layer and the imports it declares. Layer classification uses
//! **standard `globset` semantics**: a glob must match the full relative path.
//! This is intentionally stricter than `config::scope`, which auto-prefixes bare
//! patterns with `**/` to emulate bully's file-matching behavior; architecture
//! layer globs such as `src/components/**` are path-anchored by design.

use crate::arch::config::ArchConfig;
use crate::arch::resolve::normalize_path;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Extensions tried when resolving an import spec against virtual (proposed)
/// graph nodes. Mirrors [`try_extensions`]'s suffix list so a virtual node
/// keyed by the same path the filesystem resolver would produce is found.
///
/// [`try_extensions`]: crate::arch::resolve::try_extensions
const VIRTUAL_EXTENSIONS: &[&str] = &[
    "",
    ".ts",
    ".tsx",
    ".mts",
    ".cts",
    ".js",
    ".jsx",
    ".mjs",
    ".cjs",
    ".d.ts",
    "/index.ts",
    "/index.tsx",
    "/index.js",
    "/index.jsx",
    "/index.mjs",
    "/index.cjs",
];

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

    /// Merge proposed (not-yet-on-disk) files as VIRTUAL graph nodes, so
    /// `evaluate_outgoing` can resolve cross-file imports within a single
    /// atomic patch (Bug 1). Each manifest entry's proposed content overrides
    /// any disk version — that's the post-patch truth.
    ///
    /// Best-effort: a manifest entry whose content file can't be read or whose
    /// path is unparseable is silently skipped. A missing virtual node just
    /// means that target won't resolve, which is the pre-fix status quo for
    /// that edge — never worse than not having the manifest at all.
    pub fn merge_proposed(&mut self, manifest_path: &Path, config: &ArchConfig) {
        let Ok(raw) = fs::read_to_string(manifest_path) else {
            return;
        };
        for entry in parse_manifest_entries(&raw) {
            self.merge_one_virtual_node(&entry, config);
        }
    }

    /// Merge a single proposed file into the graph as a virtual node.
    fn merge_one_virtual_node(&mut self, entry: &ManifestEntry, config: &ArchConfig) {
        let Ok(source) = fs::read(&entry.content_path) else {
            return;
        };
        let canon_path = canonicalize_manifest_path(&entry.file_path, &self.root);
        // Honor `architecture.ignore` so a manifest entry for an ignored file
        // does not become a virtual node — the same single-policy answer the
        // on-disk sweep (walk_files) and the write path (evaluate_outgoing)
        // already enforce. (Final-review Minor-1.)
        let rel = canon_path.strip_prefix(&self.root).unwrap_or(&canon_path);
        if let Ok(ignore_set) = build_ignore_set(&config.ignore) {
            if ignore_set.is_match(rel) {
                return;
            }
        }
        let Some((extractor, resolver)) = crate::arch::languages::for_path(&canon_path) else {
            return;
        };
        let imports = extractor.extract(&source);
        let edges: Vec<Edge> = imports
            .into_iter()
            .filter_map(|i| {
                resolver
                    .resolve(&i.spec, &canon_path, &self.root)
                    .map(|target| Edge {
                        target,
                        spec: i.spec,
                        line: i.line,
                    })
            })
            .collect();
        let layer = self.classify(config, &canon_path);
        self.nodes.insert(canon_path, Node { layer, edges });
    }

    /// Resolve `import_spec` (relative to `importer`) to an absolute file
    /// path. Tries the filesystem first (via the language resolver), then
    /// falls back to checking the graph's VIRTUAL nodes — proposed files
    /// merged via [`merge_proposed`] that are not yet on disk. This is what
    /// lets `evaluate_outgoing` catch a cross-file import to a sibling that
    /// hasn't been written yet (Bug 1).
    ///
    /// Returns `None` if neither disk nor a virtual node matches.
    pub fn resolve_with_overlay(
        &self,
        import_spec: &str,
        importer: &Path,
        root: &Path,
        resolver: &dyn crate::arch::resolve::Resolver,
    ) -> Option<PathBuf> {
        if let Some(path) = resolver.resolve(import_spec, importer, root) {
            return Some(path);
        }
        // Filesystem didn't find it. Try virtual nodes: build the candidate
        // base path (same way the resolver does) and check each extension
        // against graph node keys.
        let base = importer.parent()?;
        let joined = base.join(import_spec);
        let normalized = normalize_path(&joined);
        for suffix in VIRTUAL_EXTENSIONS {
            let candidate = if suffix.is_empty() {
                normalized.clone()
            } else if let Some(stripped) = suffix.strip_prefix('/') {
                normalized.join(stripped)
            } else {
                PathBuf::from(format!("{}{}", normalized.display(), suffix))
            };
            // Canonicalize the candidate the same way merge_proposed does,
            // so a virtual node keyed by its canonical path is found even
            // when the importer path is non-canonical (macOS /var vs
            // /private/var).
            let canon_candidate = canonicalize_manifest_path(&candidate, &self.root);
            if self.nodes.contains_key(&canon_candidate) {
                return Some(canon_candidate);
            }
            if self.nodes.contains_key(&candidate) {
                return Some(candidate);
            }
        }
        None
    }

    /// Classify a file into a layer: first matching layer's globs win
    /// (insertion order). `None` = unlayered.
    ///
    /// Tries `strip_prefix` against both `self.root` and its canonical form,
    /// so a virtual node stored at a canonical path (`/private/var/...`) is
    /// correctly classified even when `self.root` is non-canonical (`/var/...`).
    /// This is the macOS `/var` → `/private/var` symlink case that affects
    /// `tempfile::tempdir()` paths in integration tests.
    pub fn classify(&self, config: &ArchConfig, path: &Path) -> Option<LayerId> {
        let rel = path.strip_prefix(&self.root).unwrap_or_else(|_| {
            let canon_root = self
                .root
                .canonicalize()
                .unwrap_or_else(|_| self.root.clone());
            path.strip_prefix(&canon_root).unwrap_or(path)
        });
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

/// Build a `GlobSet` from the architecture `ignore` globs using standard
/// `globset` full-path semantics.
///
/// Shared between the whole-graph sweep and the per-write path so both apply
/// `architecture.ignore` identically.
pub(crate) fn build_ignore_set(ignore: &[String]) -> anyhow::Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for glob in ignore {
        builder.add(Glob::new(glob)?);
    }
    Ok(builder.build()?)
}

/// One entry in the proposed-manifest: the absolute path of a proposed file
/// and the path to a temp file holding its proposed content.
///
/// Manifest format: tab-separated `<file_path>\t<content_path>`, one per line.
/// Paths may contain spaces (tab is the delimiter) but NOT tabs — documented
/// assumption. Blank lines are skipped.
struct ManifestEntry {
    file_path: PathBuf,
    content_path: PathBuf,
}

/// Parse the manifest into entries. Blank lines and lines without a tab are
/// skipped. A line with a tab but an empty content_path is still produced
/// (the merge will fail to read it and skip — best-effort).
fn parse_manifest_entries(raw: &str) -> Vec<ManifestEntry> {
    let mut entries = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((file_str, content_str)) = trimmed.split_once('\t') else {
            continue;
        };
        entries.push(ManifestEntry {
            file_path: PathBuf::from(file_str),
            content_path: PathBuf::from(content_str),
        });
    }
    entries
}

/// Canonicalize a manifest entry's file path so it matches the graph's
/// canonical key form. On macOS, `/var` is a symlink to `/private/var`; the
/// graph root is canonicalized via `canonicalize_through_parent` in the CLI,
/// so node keys use `/private/var/...`. Manifest entries written by the hook
/// use `os.path.normpath` (not `realpath`), so they may carry the
/// non-canonical `/var/...` prefix. Without this, a virtual node at
/// `/var/.../db.ts` wouldn't be found by `resolve_with_overlay` when the
/// candidate is built from a canonicalized importer at `/private/var/...`.
fn canonicalize_manifest_path(path: &Path, root: &Path) -> PathBuf {
    if let Ok(c) = path.canonicalize() {
        return c;
    }
    // File doesn't exist yet (pre-patch). Canonicalize through the parent
    // directory — same strategy as the CLI's `canonicalize_through_parent`.
    // This resolves macOS `/var` → `/private/var` symlink in the parent
    // dirs (which DO exist), then re-appends the missing file basename.
    if let Some(parent) = path.parent() {
        if let Ok(parent_canon) = parent.canonicalize() {
            if let Some(name) = path.file_name() {
                return parent_canon.join(name);
            }
        }
    }
    // Fallback: try root-relative join. Try both raw and canonical root.
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let rel = path
        .strip_prefix(root)
        .or_else(|_| path.strip_prefix(&root_canon))
        .unwrap_or(path);
    root_canon.join(rel)
}

/// Recursively walk `root`, returning absolute paths to every regular file.
///
/// Skips `.git` and `node_modules` directories entirely, plus any file whose
/// relative path matches one of the `ignore` globs (standard `globset`
/// semantics). Results are sorted for deterministic output.
fn walk_files(root: &Path, ignore: &[String]) -> anyhow::Result<Vec<PathBuf>> {
    let ignore_set = build_ignore_set(ignore)?;

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

    fn make_components_repo(root: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(root.join("src/components"))?;
        fs::write(root.join("src/components/App.test.ts"), "")?;
        fs::write(root.join("src/components/App.tsx"), "")?;
        Ok(())
    }

    #[test]
    fn build_ignore_set_uses_full_path_semantics() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        make_components_repo(root)?;

        let ignore = build_ignore_set(&["**/*.test.ts".into()])?;
        assert!(ignore.is_match(Path::new("src/components/App.test.ts")));
        assert!(!ignore.is_match(Path::new("src/components/App.tsx")));
        Ok(())
    }

    #[test]
    fn walk_files_skips_ignored_paths() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        make_components_repo(root)?;

        let files = walk_files(root, &["**/*.test.ts".into()])?;
        assert!(
            files.iter().any(|p| p.ends_with("App.tsx")),
            "non-ignored file is included"
        );
        assert!(
            !files.iter().any(|p| p.ends_with("App.test.ts")),
            "ignored file is excluded"
        );
        Ok(())
    }

    // --- Manifest parsing (Bug 1: env-manifest overlay) ---

    #[test]
    fn parse_manifest_basic() {
        let raw = "/repo/src/a.ts\t/tmp/content-1\n/repo/src/b.ts\t/tmp/content-2\n";
        let entries = parse_manifest_entries(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].file_path, PathBuf::from("/repo/src/a.ts"));
        assert_eq!(entries[0].content_path, PathBuf::from("/tmp/content-1"));
        assert_eq!(entries[1].file_path, PathBuf::from("/repo/src/b.ts"));
    }

    #[test]
    fn parse_manifest_skips_blank_lines() {
        let raw = "\n\n/repo/src/a.ts\t/tmp/c1\n\n\n";
        let entries = parse_manifest_entries(raw);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_path, PathBuf::from("/repo/src/a.ts"));
    }

    #[test]
    fn parse_manifest_skips_lines_without_tab() {
        let raw = "no-tab-here\n/repo/src/a.ts\t/tmp/c1\n";
        let entries = parse_manifest_entries(raw);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn parse_manifest_empty() {
        assert!(parse_manifest_entries("").is_empty());
        assert!(parse_manifest_entries("\n\n\n").is_empty());
    }

    #[test]
    fn parse_manifest_paths_with_spaces() {
        let raw = "/repo/my project/a.ts\t/tmp/content 1\n";
        let entries = parse_manifest_entries(raw);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_path, PathBuf::from("/repo/my project/a.ts"));
        assert_eq!(entries[0].content_path, PathBuf::from("/tmp/content 1"));
    }

    // --- merge_proposed (Bug 1: env-manifest overlay) ---

    fn cfg_with_rules() -> ArchConfig {
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
            rules: vec![crate::arch::config::RuleDecl {
                from: "presentation".into(),
                may_import: vec![],
            }],
            ignore: vec![],
        }
    }

    #[test]
    fn merge_proposed_adds_virtual_node_not_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/components")).unwrap();
        fs::create_dir_all(root.join("src/data")).unwrap();
        // db.ts NOT on disk.
        let db_path = root.join("src/data/db.ts");
        let content_file = root.join("db-content.txt");
        fs::write(&content_file, "export const db = 1;\n").unwrap();
        let manifest = format!("{}\t{}\n", db_path.display(), content_file.display());
        let manifest_path = root.join("manifest.tsv");
        fs::write(&manifest_path, &manifest).unwrap();

        let cfg = cfg_with_rules();
        let mut graph = DepGraph::build(root, &cfg).unwrap();
        assert!(!graph.nodes.contains_key(&db_path));
        graph.merge_proposed(&manifest_path, &cfg);
        // merge_proposed canonicalizes through parent (macOS /var → /private/var),
        // so the node key is the canonical form — not the raw db_path.
        let db_canon = canonicalize_manifest_path(&db_path, root);
        assert!(
            graph.nodes.contains_key(&db_canon),
            "merge_proposed should add db.ts as a virtual node"
        );
        let node = &graph.nodes[&db_canon];
        assert_eq!(
            node.layer,
            Some(1),
            "db.ts should be classified as data layer"
        );
    }

    #[test]
    fn merge_proposed_overrides_disk_version() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/components")).unwrap();
        fs::create_dir_all(root.join("src/data")).unwrap();
        // App.tsx must exist so the import in the proposed db.ts resolves.
        fs::write(
            root.join("src/components/App.tsx"),
            "export const App = 1;\n",
        )
        .unwrap();
        let db_path = root.join("src/data/db.ts");
        // Disk version has no imports.
        fs::write(&db_path, "// old\n").unwrap();
        // Manifest version has an import.
        let content_file = root.join("db-new.txt");
        fs::write(&content_file, "import { x } from '../components/App';\n").unwrap();
        let manifest = format!("{}\t{}\n", db_path.display(), content_file.display());
        let manifest_path = root.join("manifest.tsv");
        fs::write(&manifest_path, &manifest).unwrap();

        let cfg = cfg_with_rules();
        let mut graph = DepGraph::build(root, &cfg).unwrap();
        // Disk version has no edges.
        assert!(graph.nodes[&db_path].edges.is_empty());
        graph.merge_proposed(&manifest_path, &cfg);
        // merge_proposed canonicalizes through parent (macOS /var → /private/var),
        // so the virtual node key is the canonical form — which may differ from
        // the disk node key. The disk node still has no edges; the virtual node
        // (canonical) has the proposed import edge.
        let db_canon = canonicalize_manifest_path(&db_path, root);
        assert!(
            !graph.nodes[&db_canon].edges.is_empty(),
            "merge_proposed should override disk version with proposed content"
        );
    }

    #[test]
    fn merge_proposed_skips_missing_content_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let db_path = root.join("src/data/db.ts");
        let manifest = format!(
            "{}\t{}\n",
            db_path.display(),
            root.join("nope.txt").display()
        );
        let manifest_path = root.join("manifest.tsv");
        fs::write(&manifest_path, &manifest).unwrap();

        let cfg = cfg_with_rules();
        let mut graph = DepGraph {
            nodes: HashMap::new(),
            root: root.to_path_buf(),
        };
        graph.merge_proposed(&manifest_path, &cfg);
        assert!(
            !graph.nodes.contains_key(&db_path),
            "missing content file → entry skipped"
        );
    }

    #[test]
    fn merge_proposed_tolerates_missing_manifest_file() {
        let cfg = cfg_with_rules();
        let mut graph = DepGraph {
            nodes: HashMap::new(),
            root: PathBuf::from("/repo"),
        };
        // Should not panic.
        graph.merge_proposed(Path::new("/does/not/exist/manifest.tsv"), &cfg);
        assert!(graph.nodes.is_empty());
    }

    #[test]
    fn merge_proposed_skips_ignored_file() {
        // An ignored file in the manifest must NOT become a virtual node —
        // same single-policy answer as the on-disk sweep (walk_files) and the
        // write path (evaluate_outgoing, Bug 7). Without this, an ignored
        // *.test.ts in the manifest would be merged, which is over-blocking
        // (conservative) but inconsistent. (Final-review Minor-1.)
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/data")).unwrap();
        let test_path = root.join("src/data/db.test.ts");
        let content_file = root.join("db-content.txt");
        fs::write(&content_file, "export const db = 1;\n").unwrap();
        let manifest = format!("{}\t{}\n", test_path.display(), content_file.display());
        let manifest_path = root.join("manifest.tsv");
        fs::write(&manifest_path, &manifest).unwrap();

        let mut cfg = cfg_with_rules();
        cfg.ignore = vec!["**/*.test.ts".into()];

        let mut graph = DepGraph::build(root, &cfg).unwrap();
        graph.merge_proposed(&manifest_path, &cfg);
        let test_canon = canonicalize_manifest_path(&test_path, root);
        assert!(
            !graph.nodes.contains_key(&test_canon),
            "merge_proposed should skip an ignored file (not add it as a virtual node)"
        );
    }
}
