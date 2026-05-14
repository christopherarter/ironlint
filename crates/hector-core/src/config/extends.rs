use super::parser::parse_str;
use super::types::Config;
use anyhow::{anyhow, Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Resolve extends recursively.
///
/// Earlier ancestors are inherited; local rules win on collision. Does
/// **not** verify trust — callers that execute the resulting rules (e.g.
/// `HectorEngine::load`) must use [`resolve_trusted`] instead, so the trust
/// chain is enforced before any `script:` runs.
pub fn resolve(root: &Path) -> Result<Config> {
    let mut seen = HashSet::new();
    resolve_inner(root, &mut seen, false)
}

/// Resolve extends and verify trust at every step.
///
/// Same as [`resolve`], but additionally calls [`crate::trust::verify`] on
/// every config visited by the DFS (root and every transitive ancestor).
/// Closes the bypass where a signed child could `extends:` an unsigned parent
/// and still load — see audit P0-2.
pub fn resolve_trusted(root: &Path) -> Result<Config> {
    let mut seen = HashSet::new();
    resolve_inner(root, &mut seen, true)
}

fn resolve_inner(path: &Path, seen: &mut HashSet<PathBuf>, verify_trust: bool) -> Result<Config> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", path.display()))?;
    if !seen.insert(canonical.clone()) {
        return Err(anyhow!("extends cycle detected at {}", canonical.display()));
    }
    let content = std::fs::read_to_string(&canonical)
        .with_context(|| format!("reading {}", canonical.display()))?;
    // P2-11: detect schema v1 BEFORE trust verify so users see a clear
    // "run `hector migrate`" hint instead of "trust block missing".
    if matches!(super::parser::peek_schema_version(&content), Some(1)) {
        return Err(anyhow!(
            "{} is schema_version 1 (legacy bully); run `hector migrate` to upgrade to schema_version 2",
            canonical.display()
        ));
    }
    if verify_trust {
        crate::trust::verify(&content)
            .with_context(|| format!("trust verify for {}", canonical.display()))?;
    }
    let mut cfg = parse_str(&content)?;

    let parent_dir = canonical.parent().unwrap_or_else(|| Path::new("."));
    let extends = std::mem::take(&mut cfg.extends);
    for relative in &extends {
        let abs = parent_dir.join(relative);
        let inherited = resolve_inner(&abs, seen, verify_trust)?;
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

/// C3: resolve extends and return a side-channel mapping of every
/// surviving rule id to the canonical path of the file it was defined
/// in. Local definitions win on collision — same semantics as
/// [`resolve`] — and the origin map reflects that (the local file's
/// path is recorded for any rule the local config defined directly).
///
/// This entry point does **not** verify trust. `show-resolved-config`
/// is a read-only inspection command and operators reach for it
/// precisely when debugging an as-yet-unsigned config; gating it on
/// trust would defeat the purpose. Callers that intend to *execute*
/// rules must continue to use [`resolve_trusted`].
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
    if matches!(super::parser::peek_schema_version(&content), Some(1)) {
        return Err(anyhow!(
            "{} is schema_version 1 (legacy bully); run `hector migrate` to upgrade to schema_version 2",
            canonical.display()
        ));
    }
    let mut cfg = parse_str(&content)?;

    // Record every rule defined *directly* in this file. A closer
    // ancestor (i.e. the call frame above us in the DFS, which is
    // closer to the local config) may have already claimed the id —
    // and "closer wins" mirrors `merge_inherited`'s "local wins on
    // collision". Use `entry().or_insert_with` so an outer (closer)
    // frame's recording is never overwritten by an inner (further)
    // frame.
    for id in cfg.rules.keys() {
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
    for (id, rule) in inherited.rules {
        if !local.rules.contains_key(&id) {
            // Only record the inherited file as origin when the local
            // hasn't claimed the id. The recursive walker has already
            // recorded the *defining* file (closest ancestor); only
            // overwrite if no entry exists.
            origins
                .entry(id.clone())
                .or_insert_with(|| inherited_canonical.clone());
            local.rules.insert(id, rule);
        }
    }
    if local.llm.is_none() {
        local.llm = inherited.llm;
    }
    for g in inherited.skip {
        if !local.skip.contains(&g) {
            local.skip.push(g);
        }
    }
}
