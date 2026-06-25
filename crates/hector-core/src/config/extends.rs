use super::parser::parse_str;
use super::types::Config;
use anyhow::{anyhow, Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Resolve `extends:` recursively.
///
/// Earlier ancestors are inherited; local gates win on collision. Trust is not
/// verified here — it returns in a later plan as the out-of-repo store.
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

/// Inherited gates fill in only where the local config doesn't already define
/// them — local wins on collision.
fn merge_inherited(local: &mut Config, inherited: Config) {
    for (id, gate) in inherited.gates {
        local.gates.entry(id).or_insert(gate);
    }
}

/// Resolve `extends:` and return a per-gate origin map.
///
/// Attributes every surviving gate id to the canonical path of the file it was
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
        .with_context(|| format!("canonicalizing {}", path.display()))?;
    if !seen.insert(canonical.clone()) {
        return Err(anyhow!("extends cycle detected at {}", canonical.display()));
    }
    let content = std::fs::read_to_string(&canonical)
        .with_context(|| format!("reading {}", canonical.display()))?;
    let mut cfg = parse_str(&content)?;

    for id in cfg.gates.keys() {
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
    for (id, gate) in inherited.gates {
        if let std::collections::btree_map::Entry::Vacant(slot) = local.gates.entry(id.clone()) {
            origins
                .entry(id)
                .or_insert_with(|| inherited_canonical.clone());
            slot.insert(gate);
        }
    }
}
