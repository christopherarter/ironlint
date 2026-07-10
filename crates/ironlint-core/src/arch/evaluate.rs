//! Policy evaluator: walks the whole dependency graph and reports layer-rule
//! violations.
//!
//! For each edge, if the importer's layer has a rule whose `may_import` excludes
//! the target's layer, a `Violation` is emitted. Unlayered importers are skipped
//! (permissive by default). Unlayered targets are allowed.

use crate::arch::config::ArchConfig;
use crate::arch::graph::{DepGraph, LayerId};
use std::path::{Path, PathBuf};

/// A single architecture-rule violation: an edge whose import is forbidden by
/// the importer's layer rule.
#[derive(Debug, Clone)]
pub struct Violation {
    pub importer: PathBuf,
    pub target: PathBuf,
    pub importer_layer: LayerId,
    pub target_layer: LayerId,
    pub spec: String,
    pub line: usize,
    pub rule_from: String,
}

/// Evaluate the whole graph against layer rules.
///
/// For each edge, if the importer's layer has a rule whose `may_import`
/// excludes the target's layer, a violation is emitted. Unlayered
/// importers (no rule) are treated as permissive. Unlayered targets
/// are always allowed.
pub fn evaluate(graph: &DepGraph, config: &ArchConfig) -> Vec<Violation> {
    let mut out = Vec::new();
    for (importer, node) in &graph.nodes {
        let Some(importer_layer) = node.layer else {
            continue;
        };
        let layer_name = &config.layers[importer_layer].name;
        let Some(rule) = config.rules.iter().find(|r| r.from == *layer_name) else {
            continue; // no rule for this layer → permissive
        };
        for edge in &node.edges {
            let Some(target_node) = graph.nodes.get(&edge.target) else {
                continue;
            };
            let Some(target_layer) = target_node.layer else {
                continue;
            };
            let target_name = &config.layers[target_layer].name;
            if !rule.may_import.iter().any(|m| m == target_name) {
                out.push(Violation {
                    importer: importer.clone(),
                    target: edge.target.clone(),
                    importer_layer,
                    target_layer,
                    spec: edge.spec.clone(),
                    line: edge.line,
                    rule_from: layer_name.clone(),
                });
            }
        }
    }
    out
}

/// Evaluate the proposed content of a single file against the supplied graph.
///
/// v1 rebuilds the graph per invocation; callers pass a freshly built graph
/// so a prior accepted write is already reflected on disk.
pub fn evaluate_outgoing(
    proposed_content: &[u8],
    proposed_path: &Path,
    root: &Path,
    graph: &DepGraph,
    config: &ArchConfig,
) -> anyhow::Result<Vec<Violation>> {
    let Some((extractor, resolver)) = crate::arch::languages::for_path(proposed_path) else {
        return Ok(vec![]);
    };
    let Some(importer_layer) = graph.classify(config, proposed_path) else {
        return Ok(vec![]);
    };
    let layer_name = &config.layers[importer_layer].name;
    let Some(rule) = config.rules.iter().find(|r| r.from == *layer_name) else {
        return Ok(vec![]);
    };
    let mut violations = Vec::new();
    for import in extractor.extract(proposed_content) {
        let Some(target) = resolver.resolve(&import.spec, proposed_path, root) else {
            continue;
        };
        let Some(target_layer) = graph.nodes.get(&target).and_then(|node| node.layer) else {
            continue;
        };
        if !rule
            .may_import
            .iter()
            .any(|name| name == &config.layers[target_layer].name)
        {
            violations.push(Violation {
                importer: proposed_path.to_path_buf(),
                target,
                importer_layer,
                target_layer,
                spec: import.spec,
                line: import.line,
                rule_from: layer_name.clone(),
            });
        }
    }
    Ok(violations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch::config::RuleDecl;
    use crate::arch::graph::{DepGraph, Edge, Node};
    use std::collections::HashMap;
    use std::fs;

    fn node(id: Option<LayerId>) -> Node {
        Node::new(id)
    }

    #[test]
    fn violation_when_forbidden() {
        let graph = DepGraph {
            nodes: HashMap::from_iter([
                (
                    PathBuf::from("/a"),
                    Node {
                        layer: Some(0),
                        edges: vec![Edge {
                            target: PathBuf::from("/b"),
                            spec: "'./b'".into(),
                            line: 1,
                        }],
                    },
                ),
                (PathBuf::from("/b"), node(Some(1))),
            ]),
            root: PathBuf::from("/"),
        };
        let config = ArchConfig {
            layers: vec![
                crate::arch::config::LayerDecl {
                    name: "presentation".into(),
                    globs: vec![],
                },
                crate::arch::config::LayerDecl {
                    name: "data".into(),
                    globs: vec![],
                },
            ],
            rules: vec![RuleDecl {
                from: "presentation".into(),
                may_import: vec![],
            }],
            ignore: vec![],
        };
        let violations = evaluate(&graph, &config);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule_from, "presentation");
    }

    #[test]
    fn no_violation_when_permitted() {
        let graph = DepGraph {
            nodes: HashMap::from_iter([
                (
                    PathBuf::from("/a"),
                    Node {
                        layer: Some(0),
                        edges: vec![Edge {
                            target: PathBuf::from("/b"),
                            spec: "'./b'".into(),
                            line: 1,
                        }],
                    },
                ),
                (PathBuf::from("/b"), node(Some(1))),
            ]),
            root: PathBuf::from("/"),
        };
        let config = ArchConfig {
            layers: vec![
                crate::arch::config::LayerDecl {
                    name: "presentation".into(),
                    globs: vec![],
                },
                crate::arch::config::LayerDecl {
                    name: "data".into(),
                    globs: vec![],
                },
            ],
            rules: vec![RuleDecl {
                from: "presentation".into(),
                may_import: vec!["data".into()],
            }],
            ignore: vec![],
        };
        let violations = evaluate(&graph, &config);
        assert!(violations.is_empty());
    }

    #[test]
    fn skip_unlayered_importer() {
        let graph = DepGraph {
            nodes: HashMap::from_iter([
                (PathBuf::from("/a"), node(None)), // unlayered
            ]),
            root: PathBuf::from("/"),
        };
        let config = ArchConfig {
            layers: vec![crate::arch::config::LayerDecl {
                name: "presentation".into(),
                globs: vec![],
            }],
            rules: vec![RuleDecl {
                from: "presentation".into(),
                may_import: vec![],
            }],
            ignore: vec![],
        };
        assert!(evaluate(&graph, &config).is_empty());
    }

    #[test]
    fn skip_no_rule_for_layer() {
        let graph = DepGraph {
            nodes: HashMap::from_iter([
                (
                    PathBuf::from("/a"),
                    Node {
                        layer: Some(0),
                        edges: vec![Edge {
                            target: PathBuf::from("/b"),
                            spec: "'./b'".into(),
                            line: 1,
                        }],
                    },
                ),
                (PathBuf::from("/b"), node(Some(0))),
            ]),
            root: PathBuf::from("/"),
        };
        let config = ArchConfig {
            layers: vec![crate::arch::config::LayerDecl {
                name: "presentation".into(),
                globs: vec![],
            }],
            rules: vec![], // no rule for presentation
            ignore: vec![],
        };
        assert!(evaluate(&graph, &config).is_empty());
    }

    #[test]
    fn skip_unlayered_target() {
        let graph = DepGraph {
            nodes: HashMap::from_iter([
                (
                    PathBuf::from("/a"),
                    Node {
                        layer: Some(0),
                        edges: vec![Edge {
                            target: PathBuf::from("/b"),
                            spec: "'./b'".into(),
                            line: 1,
                        }],
                    },
                ),
                (PathBuf::from("/b"), node(None)), // unlayered target
            ]),
            root: PathBuf::from("/"),
        };
        let config = ArchConfig {
            layers: vec![crate::arch::config::LayerDecl {
                name: "presentation".into(),
                globs: vec![],
            }],
            rules: vec![RuleDecl {
                from: "presentation".into(),
                may_import: vec![],
            }],
            ignore: vec![],
        };
        assert!(evaluate(&graph, &config).is_empty());
    }

    #[test]
    fn empty_graph_no_violations() {
        let graph = DepGraph {
            nodes: HashMap::new(),
            root: PathBuf::from("/"),
        };
        let config = ArchConfig {
            layers: vec![crate::arch::config::LayerDecl {
                name: "presentation".into(),
                globs: vec![],
            }],
            rules: vec![],
            ignore: vec![],
        };
        assert!(evaluate(&graph, &config).is_empty());
    }

    #[test]
    fn unsupported_language_returns_empty() {
        let graph = DepGraph {
            nodes: HashMap::new(),
            root: PathBuf::from("/repo"),
        };
        let config = ArchConfig {
            layers: vec![crate::arch::config::LayerDecl {
                name: "presentation".into(),
                globs: vec![],
            }],
            rules: vec![],
            ignore: vec![],
        };
        assert!(evaluate_outgoing(
            b"import { x } from './x';",
            Path::new("/repo/README.md"),
            Path::new("/repo"),
            &graph,
            &config
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn unlayered_importer_returns_empty() {
        let graph = DepGraph {
            nodes: HashMap::from_iter([(
                PathBuf::from("/repo/src/lib.ts"),
                Node {
                    layer: None,
                    edges: vec![],
                },
            )]),
            root: PathBuf::from("/repo"),
        };
        let config = ArchConfig {
            layers: vec![crate::arch::config::LayerDecl {
                name: "presentation".into(),
                globs: vec!["src/components/**".into()],
            }],
            rules: vec![RuleDecl {
                from: "presentation".into(),
                may_import: vec![],
            }],
            ignore: vec![],
        };
        assert!(evaluate_outgoing(
            b"import { x } from '../data/x';",
            Path::new("/repo/src/lib.ts"),
            Path::new("/repo"),
            &graph,
            &config
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn skip_edge_when_target_not_in_graph() {
        let graph = DepGraph {
            nodes: HashMap::from_iter([(
                PathBuf::from("/a"),
                Node {
                    layer: Some(0),
                    edges: vec![Edge {
                        target: PathBuf::from("/b"), // /b is NOT in graph.nodes
                        spec: "'./b'".into(),
                        line: 1,
                    }],
                },
            )]),
            root: PathBuf::from("/"),
        };
        let config = ArchConfig {
            layers: vec![crate::arch::config::LayerDecl {
                name: "presentation".into(),
                globs: vec![],
            }],
            rules: vec![RuleDecl {
                from: "presentation".into(),
                may_import: vec![],
            }],
            ignore: vec![],
        };
        // The edge targets /b which has no node in the graph,
        // so the continue branch is taken and no violation is emitted.
        assert!(evaluate(&graph, &config).is_empty());
    }

    #[test]
    fn no_rule_for_importer_layer_returns_empty() {
        // Importer classifies into "present" layer but the only rule is for
        // a different layer, so `find` returns None → line 83 returns early.
        let graph = DepGraph {
            nodes: HashMap::from_iter([(PathBuf::from("/repo/src/components/lib.ts"), node(None))]),
            root: PathBuf::from("/repo"),
        };
        let config = ArchConfig {
            layers: vec![
                crate::arch::config::LayerDecl {
                    name: "data".into(),
                    globs: vec!["src/data/**".into()],
                },
                crate::arch::config::LayerDecl {
                    name: "present".into(),
                    globs: vec!["src/components/**".into()],
                },
            ],
            rules: vec![RuleDecl {
                from: "data".into(),
                may_import: vec![],
            }],
            ignore: vec![],
        };
        let violations = evaluate_outgoing(
            b"import { x } from './other';",
            Path::new("/repo/src/components/lib.ts"),
            Path::new("/repo"),
            &graph,
            &config,
        )
        .unwrap();
        assert!(
            violations.is_empty(),
            "no rule for 'present' layer → early return"
        );
    }

    #[test]
    fn unresolved_import_continues() {
        // Import spec that does NOT resolve to any file on disk,
        // resolver.resolve returns None → line 88 continues.
        let graph = DepGraph {
            nodes: HashMap::from_iter([
                (
                    PathBuf::from("/repo/src/components/lib.ts"),
                    Node {
                        layer: Some(0),
                        edges: vec![],
                    },
                ),
                (
                    PathBuf::from("/repo/src/components/other.ts"),
                    node(Some(0)),
                ),
            ]),
            root: PathBuf::from("/repo"),
        };
        let config = ArchConfig {
            layers: vec![crate::arch::config::LayerDecl {
                name: "present".into(),
                globs: vec!["src/components/**".into()],
            }],
            rules: vec![RuleDecl {
                from: "present".into(),
                may_import: vec!["data".into()],
            }],
            ignore: vec![],
        };
        let violations = evaluate_outgoing(
            b"import { x } from '../src/nonexistent_z9x8y7';",
            Path::new("/repo/src/components/lib.ts"),
            Path::new("/repo"),
            &graph,
            &config,
        )
        .unwrap();
        assert!(
            violations.is_empty(),
            "unresolved import → continue, no violation"
        );
    }

    #[test]
    fn target_unlayered_returns_empty() {
        // Import resolves to a file on disk, but that file is not
        // classified into any layer (no rule matches its path).
        // resolver.resolve returns Some → graph.nodes.get() returns Some →
        // node.layer is None → line 91 continues.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let components = root.join("src/components");
        fs::create_dir_all(&components).unwrap();
        let importer = components.join("lib.ts");
        fs::write(&importer, "// importer").unwrap();

        // From src/components/lib.ts, '../external_z9x8y7' resolves to
        // src/external_z9x8y7.ts.
        let target = root.join("src/external_z9x8y7.ts");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "// placeholder").unwrap();

        let graph = DepGraph {
            nodes: HashMap::from_iter([
                (
                    importer.clone(),
                    Node {
                        layer: Some(0),
                        edges: vec![],
                    },
                ),
                // Target exists in graph but has no layer assignment.
                (target.clone(), node(None)),
            ]),
            root: root.to_path_buf(),
        };
        let config = ArchConfig {
            layers: vec![crate::arch::config::LayerDecl {
                name: "present".into(),
                globs: vec!["src/components/**".into()],
            }],
            rules: vec![RuleDecl {
                from: "present".into(),
                may_import: vec!["data".into()],
            }],
            ignore: vec![],
        };
        let violations = evaluate_outgoing(
            b"import { x } from '../external_z9x8y7';",
            &importer,
            root,
            &graph,
            &config,
        )
        .unwrap();
        assert!(
            violations.is_empty(),
            "target unlayered → continue, no violation"
        );
    }
}
