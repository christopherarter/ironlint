//! Bare `ironlint check` — the repo-wide sweep.
//!
//! Dispatch model (each check runs exactly once per sweep, keyed by
//! lifecycle):
//! - checks whose `on:` includes `pre-commit` are BATCHED: one `check_set`
//!   invocation per check over the full walked set ($IRONLINT_FILES
//!   populated, stdin empty) — one process spawn per check, not per file;
//! - write-only checks run per file with on-disk content on stdin,
//!   exactly like `--diff` mode.
//!
//! A dual-lifecycle check (`on: [write, pre-commit]`) is batched only, so it
//! is never double-run.

use crate::cli::OutputFormat;
use crate::commands::check::{check_files_individually, emit, exit_code, print_explain};
use crate::commands::error_report::emit_error;
use anyhow::Result;
use ironlint_core::config::{Check, Lifecycle};
use ironlint_core::runner::IronLintEngine;
use ironlint_core::verdict::Verdict;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Which sweep phase each check id belongs to. The split is disjoint.
pub(crate) struct SweepClasses {
    pub per_file: HashSet<String>,
    /// Consumed by Task 4's batched (`check_set`) phase; only read by unit
    /// tests today, so it's genuinely dead from clippy's (non-test) vantage
    /// point until that phase lands.
    #[allow(dead_code)]
    pub batched: HashSet<String>,
}

/// Split the resolved check map into sweep phases, honoring the user's
/// `--check` filter (empty filter = all checks).
pub(crate) fn classify_checks(
    checks: &BTreeMap<String, Check>,
    user_filter: &HashSet<String>,
) -> SweepClasses {
    let mut per_file = HashSet::new();
    let mut batched = HashSet::new();
    for (id, check) in checks {
        if !user_filter.is_empty() && !user_filter.contains(id) {
            continue;
        }
        if check.on.contains(&Lifecycle::PreCommit) {
            batched.insert(id.clone());
        } else {
            per_file.insert(id.clone());
        }
    }
    SweepClasses { per_file, batched }
}

/// Walk `root` collecting the sweep's candidate files: regular files only,
/// hidden entries skipped, `.gitignore`/`.ignore` honored even outside a git
/// repo (`require_git(false)` — the sweep, not git, defines the working set).
/// Paths are relativized to the process cwd when possible so verdict output
/// stays repo-relative, and sorted so output and CI diffs are deterministic.
pub(crate) fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let root = root.canonicalize()?;
    let cwd = std::env::current_dir()?.canonicalize()?;
    let mut files: Vec<PathBuf> = ignore::WalkBuilder::new(&root)
        .require_git(false)
        .build()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| {
            let p = e.into_path();
            p.strip_prefix(&cwd).map(Path::to_path_buf).unwrap_or(p)
        })
        .collect();
    files.sort();
    Ok(files)
}

/// Bare-`check` orchestration. `engine` arrives loaded with event `write`,
/// trust-gated, and with the user's `--check` filter already validated.
pub(crate) fn run(
    engine: &mut IronLintEngine,
    config: &Path,
    user_checks: &HashSet<String>,
    format: OutputFormat,
    explain: bool,
    require_match: bool,
) -> Result<i32> {
    let parent = config.parent().unwrap_or_else(|| Path::new("."));
    let root = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let files = walk_files(root)?;
    let classes = classify_checks(engine.checks(), user_checks);

    let mut blocks = Vec::new();
    let mut errors = Vec::new();
    let mut passed = Vec::new();
    let mut explains = Vec::new();
    let mut elapsed: u64 = 0;

    // Phase 1 — write-only checks, one invocation per matching file.
    if !classes.per_file.is_empty() {
        engine.set_check_filter(classes.per_file.clone());
        // Prune to files at least one phase-1 check scopes to; without this,
        // every walked file would produce a no-op engine call and a
        // telemetry row.
        let scoped: Vec<PathBuf> = files
            .iter()
            .filter(|f| {
                classes
                    .per_file
                    .iter()
                    .any(|id| engine.check_matches_path(id, f))
            })
            .cloned()
            .collect();
        match check_files_individually(engine, &scoped) {
            Ok(folded) => {
                blocks.extend(folded.blocks);
                errors.extend(folded.errors);
                passed.extend(folded.passed);
                explains.extend(folded.explains);
                elapsed = elapsed.saturating_add(folded.elapsed_ms);
            }
            Err(e) => return Ok(emit_error(format, &format!("{e:#}"), 1)),
        }
    }

    let verdict = Verdict::from_outcomes(blocks, errors, passed, elapsed);
    if explain {
        print_explain(&explains);
    }
    emit(&verdict, format)?;
    Ok(exit_code(&verdict, require_match))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironlint_core::config::{Check, Lifecycle};
    use std::collections::{BTreeMap, HashSet};

    fn check_on(on: Vec<Lifecycle>) -> Check {
        Check {
            files: vec!["*.md".to_string()],
            run: Some("exit 0".to_string()),
            steps: None,
            on,
            name: None,
        }
    }

    #[test]
    fn classify_splits_write_only_from_batchable() {
        let mut checks = BTreeMap::new();
        checks.insert("w".to_string(), check_on(vec![Lifecycle::Write]));
        checks.insert("p".to_string(), check_on(vec![Lifecycle::PreCommit]));
        checks.insert(
            "both".to_string(),
            check_on(vec![Lifecycle::Write, Lifecycle::PreCommit]),
        );
        let classes = classify_checks(&checks, &HashSet::new());
        assert_eq!(classes.per_file, HashSet::from(["w".to_string()]));
        assert_eq!(
            classes.batched,
            HashSet::from(["p".to_string(), "both".to_string()])
        );
    }

    #[test]
    fn classify_honors_user_filter() {
        let mut checks = BTreeMap::new();
        checks.insert("w".to_string(), check_on(vec![Lifecycle::Write]));
        checks.insert("p".to_string(), check_on(vec![Lifecycle::PreCommit]));
        let filter = HashSet::from(["p".to_string()]);
        let classes = classify_checks(&checks, &filter);
        assert!(classes.per_file.is_empty());
        assert_eq!(classes.batched, HashSet::from(["p".to_string()]));
    }

    #[test]
    fn walk_collects_sorted_files_skipping_hidden_and_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("b/.hidden")).unwrap();
        std::fs::create_dir_all(root.join("vendor")).unwrap();
        std::fs::write(root.join("z.md"), "z").unwrap();
        std::fs::write(root.join("b/a.md"), "a").unwrap();
        std::fs::write(root.join("b/.hidden/skip.md"), "s").unwrap();
        // `.ignore` (not `.gitignore`) so the rule holds without a git repo,
        // though require_git(false) makes .gitignore work here too.
        std::fs::write(root.join(".ignore"), "vendor/\n").unwrap();
        std::fs::write(root.join("vendor/skip.md"), "s").unwrap();

        let files = walk_files(root).unwrap();
        let canon = root.canonicalize().unwrap();
        assert_eq!(files, vec![canon.join("b/a.md"), canon.join("z.md")]);
    }
}
