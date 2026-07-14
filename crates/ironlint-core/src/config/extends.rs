use super::parser::parse_str;
use super::types::Config;
use anyhow::{anyhow, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// Resolve `extends:` recursively.
///
/// Earlier ancestors are inherited; local checks win on collision. Trust is not
/// verified here — it returns in a later plan as the out-of-repo store.
pub fn resolve(root: &Path) -> Result<Config> {
    let mut seen = HashSet::new();
    resolve_inner(root, &mut seen)
}

fn resolve_inner(path: &Path, seen: &mut HashSet<PathBuf>) -> Result<Config> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))?;
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

/// Return the canonical paths of every config file in the `extends:` closure
/// rooted at `root` — `root` itself plus every transitively-extended file —
/// deduped and deterministically sorted.
///
/// Reuses the same path-based cycle detection as [`resolve`]: a config that
/// (transitively) extends an ancestor on the current path is an error. The trust
/// layer folds this whole set into the blessed hash so editing any extended file
/// invalidates trust.
pub fn resolve_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut on_path = HashSet::new();
    let mut all = BTreeSet::new();
    collect_paths(root, &mut on_path, &mut all)?;
    Ok(all.into_iter().collect())
}

fn collect_paths(
    path: &Path,
    on_path: &mut HashSet<PathBuf>,
    all: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))?;
    if !on_path.insert(canonical.clone()) {
        return Err(anyhow!("extends cycle detected at {}", canonical.display()));
    }
    all.insert(canonical.clone());
    let content = std::fs::read_to_string(&canonical)
        .with_context(|| format!("reading {}", canonical.display()))?;
    let cfg = parse_str(&content)?;
    let parent_dir = canonical.parent().unwrap_or_else(|| Path::new("."));
    for relative in &cfg.extends {
        collect_paths(&parent_dir.join(relative), on_path, all)?;
    }
    on_path.remove(&canonical);
    Ok(())
}

/// Inherited checks fill in only where the local config doesn't already define
/// them — local wins on collision. `execution` (e.g. `timeout_secs`) merges the
/// same way: a local config that sets no `execution:` block inherits the
/// nearest ancestor's; an explicit local block always wins.
fn merge_inherited(local: &mut Config, inherited: Config) {
    for (id, check) in inherited.checks {
        local.checks.entry(id).or_insert(check);
    }
    local.execution = local.execution.take().or(inherited.execution);
}

/// Resolve `extends:` and return a per-check origin map.
///
/// Attributes every surviving check id to the canonical path of the file it was
/// defined in. Local definitions win on collision and the origin map reflects
/// that. Read-only inspection (`show-resolved-config`); no trust verification.
pub fn resolve_with_origin(root: &Path) -> Result<(Config, BTreeMap<String, PathBuf>)> {
    let mut seen = HashSet::new();
    let mut origins: BTreeMap<String, PathBuf> = BTreeMap::new();
    let cfg = resolve_inner_with_origin(root, &mut seen, &mut origins)?;
    Ok((cfg, origins))
}

fn resolve_inner_with_origin(
    path: &Path,
    seen: &mut HashSet<PathBuf>,
    origins: &mut BTreeMap<String, PathBuf>,
) -> Result<Config> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))?;
    if !seen.insert(canonical.clone()) {
        return Err(anyhow!("extends cycle detected at {}", canonical.display()));
    }
    let content = std::fs::read_to_string(&canonical)
        .with_context(|| format!("reading {}", canonical.display()))?;
    let mut cfg = parse_str(&content)?;

    for id in cfg.checks.keys() {
        origins
            .entry(id.clone())
            .or_insert_with(|| canonical.clone());
    }

    let parent_dir = canonical.parent().unwrap_or_else(|| Path::new("."));
    let extends = std::mem::take(&mut cfg.extends);
    for relative in &extends {
        let abs = parent_dir.join(relative);
        let inherited = resolve_inner_with_origin(&abs, seen, origins)?;
        merge_inherited_with_origin(&mut cfg, inherited, origins, &abs);
    }
    seen.remove(&canonical);
    Ok(cfg)
}

fn merge_inherited_with_origin(
    local: &mut Config,
    inherited: Config,
    origins: &mut BTreeMap<String, PathBuf>,
    inherited_from: &Path,
) {
    let inherited_canonical = inherited_from
        .canonicalize()
        .unwrap_or_else(|_| inherited_from.to_path_buf());
    let inherited_execution = inherited.execution;
    for (id, check) in inherited.checks {
        if let std::collections::btree_map::Entry::Vacant(slot) = local.checks.entry(id.clone()) {
            origins
                .entry(id)
                .or_insert_with(|| inherited_canonical.clone());
            slot.insert(check);
        }
    }
    local.execution = local.execution.take().or(inherited_execution);
}
