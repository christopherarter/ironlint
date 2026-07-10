use anyhow::{Context, Result};
use ironlint_core::arch::config::ArchConfig;
use ironlint_core::arch::engine::{ArchEngine, ArchOutcome};
use ironlint_core::arch::evaluate::Violation;
use ironlint_core::arch::graph::DepGraph;
use std::io::Read;
use std::path::{Path, PathBuf};

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

fn load_config(layers: Option<PathBuf>, root: Option<PathBuf>) -> Result<(PathBuf, ArchConfig)> {
    let root = root.unwrap_or_else(|| std::env::current_dir().unwrap());
    let root = canonicalize_through_parent(&root);
    let layers_path = layers.unwrap_or_else(|| root.join(".ironlint").join("arch.yml"));
    let content = std::fs::read_to_string(&layers_path)
        .with_context(|| format!("reading layers file {}", layers_path.display()))?;
    let config: ArchConfig = serde_yaml::from_str(&content)
        .with_context(|| format!("parsing layers file {}", layers_path.display()))?;
    config
        .validate()
        .with_context(|| format!("validating layers file {}", layers_path.display()))?;
    Ok((root, config))
}

pub fn run(sub: crate::cli::ArchSub) -> Result<i32> {
    let result = (|| -> Result<i32> {
        match sub {
            crate::cli::ArchSub::Check {
                layers,
                root,
                event,
                file,
            } => {
                let (root, config) = load_config(layers, root)?;
                run_check(&root, &config, event.as_deref(), file)
            }
            crate::cli::ArchSub::Graph {
                layers,
                root,
                dot: _,
                json,
            } => {
                let (root, config) = load_config(layers, root)?;
                run_graph(&root, &config, json)
            }
            crate::cli::ArchSub::Why { path, layers, root } => {
                let (root, config) = load_config(layers, root)?;
                run_why(&root, &config, &path)
            }
        }
    })();
    match result {
        Ok(exit) => Ok(exit),
        Err(error) => {
            eprintln!("ironlint arch: {error:#}");
            Ok(3)
        }
    }
}

fn run_check(
    root: &Path,
    config: &ArchConfig,
    event: Option<&str>,
    file: Option<PathBuf>,
) -> Result<i32> {
    let outcome = if event == Some("write") {
        match file {
            Some(path) => {
                let mut content = Vec::new();
                std::io::stdin()
                    .read_to_end(&mut content)
                    .context("reading proposed content from stdin")?;
                let absolute = if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                };
                let absolute = canonicalize_through_parent(&absolute);
                ArchEngine::check_write(root, config, &absolute, &content)
            }
            None => ArchEngine::check_whole(root, config),
        }
    } else {
        ArchEngine::check_whole(root, config)
    };
    Ok(map_check_outcome(outcome))
}

fn map_check_outcome(outcome: ArchOutcome) -> i32 {
    match outcome {
        ArchOutcome::Pass => 0,
        ArchOutcome::Block { violations } => {
            for violation in violations {
                print_violation(&violation);
            }
            2
        }
        ArchOutcome::InternalError(error) => {
            eprintln!("ironlint arch check: {error}");
            3
        }
    }
}

fn run_graph(root: &Path, config: &ArchConfig, json: bool) -> Result<i32> {
    let graph = ArchEngine::graph(root, config).map_err(|e| anyhow::anyhow!(e))?;
    if json {
        print_graph_json(&graph)?;
    } else {
        print_graph_dot(&graph);
    }
    Ok(0)
}

fn run_why(root: &Path, config: &ArchConfig, path: &Path) -> Result<i32> {
    let violations = ArchEngine::why(root, config, path).map_err(|e| anyhow::anyhow!(e))?;
    for violation in violations {
        print_violation(&violation);
    }
    Ok(0)
}

fn print_violation(v: &Violation) {
    println!(
        "{}:{} -> {} (layer {} may not import {})",
        v.importer.display(),
        v.line,
        v.target.display(),
        v.rule_from,
        v.spec
    );
}

fn print_graph_dot(graph: &DepGraph) {
    println!("digraph arch {{");
    for (importer, node) in &graph.nodes {
        for edge in &node.edges {
            println!(
                "    \"{}\" -> \"{}\";",
                importer.display(),
                edge.target.display()
            );
        }
    }
    println!("}}");
}

fn print_graph_json(graph: &DepGraph) -> Result<()> {
    let edges: Vec<serde_json::Map<String, serde_json::Value>> = graph
        .nodes
        .iter()
        .flat_map(|(importer, node)| {
            node.edges.iter().map(|edge| {
                let mut m = serde_json::Map::new();
                m.insert(
                    "importer".to_string(),
                    serde_json::Value::String(importer.to_string_lossy().into_owned()),
                );
                m.insert(
                    "target".to_string(),
                    serde_json::Value::String(edge.target.to_string_lossy().into_owned()),
                );
                m.insert(
                    "spec".to_string(),
                    serde_json::Value::String(edge.spec.clone()),
                );
                m.insert("line".to_string(), serde_json::Value::from(edge.line));
                m
            })
        })
        .collect();
    let mut root = serde_json::Map::new();
    root.insert(
        "edges".to_string(),
        serde_json::Value::Array(edges.into_iter().map(serde_json::Value::Object).collect()),
    );
    let value = serde_json::Value::Object(root);
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}
