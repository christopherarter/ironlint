//! ArchEngine entry points: whole-graph sweep, per-write outgoing check,
//! raw graph access, and violation explanations.

use crate::arch::config::ArchConfig;
use crate::arch::evaluate::{evaluate, evaluate_outgoing, Violation};
use crate::arch::graph::DepGraph;
use std::path::Path;

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
    pub fn check_write(
        root: &Path,
        config: &ArchConfig,
        proposed: &Path,
        content: &[u8],
    ) -> ArchOutcome {
        let graph = match DepGraph::build(root, config) {
            Ok(g) => g,
            Err(e) => return ArchOutcome::InternalError(format!("{e:#}")),
        };
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
        let g = DepGraph::build(root, config).map_err(|e| format!("{e:#}"))?;
        let requested = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        Ok(evaluate(&g, config)
            .into_iter()
            .filter(|v| v.importer == requested)
            .collect())
    }
}
