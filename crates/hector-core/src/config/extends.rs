use super::parser::parse_str;
use super::types::Config;
use anyhow::{anyhow, Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Resolve extends recursively. Earlier ancestors are inherited; local rules win on collision.
pub fn resolve(root: &Path) -> Result<Config> {
    let mut seen = HashSet::new();
    resolve_inner(root, &mut seen)
}

fn resolve_inner(path: &Path, seen: &mut HashSet<PathBuf>) -> Result<Config> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", path.display()))?;
    if !seen.insert(canonical.clone()) {
        return Err(anyhow!("extends cycle detected at {}", canonical.display()));
    }
    let content = std::fs::read_to_string(&canonical)
        .with_context(|| format!("reading {}", canonical.display()))?;
    let mut cfg = parse_str(&content)?;

    let parent_dir = canonical.parent().unwrap_or_else(|| Path::new("."));
    let extends = std::mem::take(&mut cfg.extends);
    for relative in &extends {
        let abs = parent_dir.join(relative);
        let inherited = resolve_inner(&abs, seen)?;
        merge_inherited(&mut cfg, inherited);
    }
    seen.remove(&canonical);
    Ok(cfg)
}

fn merge_inherited(local: &mut Config, inherited: Config) {
    // Inherited rules fill in only where local doesn't already define them.
    for (id, rule) in inherited.rules {
        local.rules.entry(id).or_insert(rule);
    }
    if local.llm.is_none() {
        local.llm = inherited.llm;
    }
    // Skip entries are additive — the union of every config in the extends
    // chain is what fires. Order doesn't matter (globs are unordered set
    // semantics), so we just append (deduped).
    for g in inherited.skip {
        if !local.skip.contains(&g) {
            local.skip.push(g);
        }
    }
    // trust block is per-config; never inherited.
}
