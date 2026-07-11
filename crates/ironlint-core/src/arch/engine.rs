//! ArchEngine entry points: whole-graph sweep, per-write outgoing check,
//! raw graph access, and violation explanations.

use crate::arch::config::ArchConfig;
use crate::arch::evaluate::{evaluate, evaluate_outgoing, Violation};
use crate::arch::graph::DepGraph;
use std::path::{Path, PathBuf};

/// Resolve a path to its canonical form, falling back to the raw path when
/// any intermediate component cannot be resolved on disk.
fn canonicalize_through_parent(path: &Path) -> PathBuf {
    if let Ok(c) = path.canonicalize() {
        return c;
    }
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path.to_path_buf();
    while let Some(name) = cursor.file_name() {
        suffix.push(name.to_os_string());
        if !cursor.pop() {
            break;
        }
        if let Ok(c) = cursor.canonicalize() {
            let mut out = c;
            for seg in suffix.into_iter().rev() {
                out.push(seg);
            }
            return out;
        }
    }
    path.to_path_buf()
}

/// Outcome of an architecture check.
#[derive(Debug, Clone)]
pub enum ArchOutcome {
    Pass,
    Block { violations: Vec<Violation> },
    InternalError(String),
}

/// High-level architecture-enforcement engine.
pub struct ArchEngine;

impl ArchEngine {
    /// Whole-graph check (pre-commit/sweep).
    pub fn check_whole(root: &Path, config: &ArchConfig) -> ArchOutcome {
        let graph = match DepGraph::build(root, config) {
            Ok(g) => g,
            Err(e) => return ArchOutcome::InternalError(format!("{e:#}")),
        };
        let violations = evaluate(&graph, config);
        if violations.is_empty() {
            ArchOutcome::Pass
        } else {
            ArchOutcome::Block { violations }
        }
    }

    /// Per-write check (outgoing only). `content` is the proposed file content.
    /// `proposed_manifest` — when `Some` — is a path to a manifest of ALL
    /// sibling proposed files in the same atomic patch (tab-separated
    /// `file_path\tcontent_path` lines). The arch engine merges those as
    /// VIRTUAL graph nodes before resolving the proposed file's outgoing
    /// imports, so a cross-file import to a not-yet-on-disk file is caught
    /// instead of silently dropped (Bug 1).
    pub fn check_write(
        root: &Path,
        config: &ArchConfig,
        proposed: &Path,
        content: &[u8],
        proposed_manifest: Option<&Path>,
    ) -> ArchOutcome {
        let mut graph = match DepGraph::build(root, config) {
            Ok(g) => g,
            Err(e) => return ArchOutcome::InternalError(format!("{e:#}")),
        };
        if let Some(manifest) = proposed_manifest {
            graph.merge_proposed(manifest, config);
        }
        match evaluate_outgoing(content, proposed, root, &graph, config) {
            Ok(v) if v.is_empty() => ArchOutcome::Pass,
            Ok(v) => ArchOutcome::Block { violations: v },
            // Defensive: evaluate_outgoing has no fallible path today (all early
            // returns are `Ok(vec![])` and the final return is `Ok(violations)`),
            // but keeping this arm so a future fallible addition surfaces as
            // InternalError rather than panicking on the unwrap.
            Err(e) => ArchOutcome::InternalError(format!("{e:#}")),
        }
    }

    /// Build and return the dependency graph for inspection.
    pub fn graph(root: &Path, config: &ArchConfig) -> Result<DepGraph, String> {
        DepGraph::build(root, config).map_err(|e| format!("{e:#}"))
    }

    /// Return every architecture violation whose importer is `path`.
    pub fn why(root: &Path, config: &ArchConfig, path: &Path) -> Result<Vec<Violation>, String> {
        // Graph keys are canonical (they come from walking `root`), so canonicalize
        // both `root` and the requested path before comparing. This keeps `why`
        // correct when callers pass symlinked paths such as `/tmp/...` on macOS,
        // where `/tmp` resolves to `/private/tmp`.
        let root = canonicalize_through_parent(root);
        let g = DepGraph::build(&root, config).map_err(|e| format!("{e:#}"))?;
        let requested = if path.is_absolute() {
            canonicalize_through_parent(path)
        } else {
            canonicalize_through_parent(&root.join(path))
        };
        Ok(evaluate(&g, config)
            .into_iter()
            .filter(|v| v.importer == requested)
            .collect())
    }
}
