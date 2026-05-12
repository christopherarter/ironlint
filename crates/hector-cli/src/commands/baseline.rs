use anyhow::Result;
use hector_core::baseline::Baseline;
use hector_core::runner::{CheckInput, HectorEngine};
use ignore::WalkBuilder;
use rayon::prelude::*;
use std::path::Path;
use std::sync::Mutex;

pub fn run(config: &Path, scan_glob: Option<String>) -> Result<i32> {
    let engine = HectorEngine::load(config)?;
    let dir = config
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let baseline_path = dir.join(".hector/baseline.json");
    let baseline = Mutex::new(Baseline::load(&baseline_path)?);

    let pattern = scan_glob.unwrap_or_else(|| "**/*".to_string());
    let glob = globset::Glob::new(&pattern)?.compile_matcher();

    // `ignore::WalkBuilder` with `standard_filters` honors `.gitignore`,
    // `.ignore`, `.git/info/exclude`, and global excludes. Without this, a
    // walkdir-based scan reads every file under target/, node_modules/, and
    // any other build-output or vendored directory the project has chosen
    // to ignore — which on a real repo OOMs or takes minutes (P0-10). The
    // built-in `SkipMatcher` in core still short-circuits engine.check, but
    // by then we've already done the I/O.
    //
    // `require_git(false)` lets `.gitignore` apply even when the project
    // hasn't been `git init`ed yet (and keeps the test fixtures honest —
    // tempdirs aren't repos).
    let paths: Vec<_> = WalkBuilder::new(dir)
        .standard_filters(true)
        .require_git(false)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(ignore::DirEntry::into_path)
        .filter(|p| {
            let rel = p.strip_prefix(dir).unwrap_or(p);
            glob.is_match(rel)
        })
        .collect();

    // `HectorEngine` is `Send + Sync` because `LlmClient: Send + Sync` and
    // every other field is owned data — parallelising the per-file check is
    // safe. Violations from each file are accumulated under a single Mutex
    // so the final baseline is deterministic regardless of thread order.
    paths.par_iter().for_each(|path| {
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        // E1: clone the file content so the engine can take ownership
        // while we still have a borrow to hash for the line_sha256.
        let content_for_hash = content.clone();
        let Ok(verdict) = engine.check(CheckInput::File {
            path: path.clone(),
            content,
        }) else {
            return;
        };
        let mut bl = baseline.lock().unwrap();
        for v in verdict.violations {
            bl.add_with_content(&v, Some(&content_for_hash));
        }
    });

    let baseline = baseline.into_inner().unwrap();
    baseline.save(&baseline_path)?;
    println!(
        "baseline written: {} ({} entries)",
        baseline_path.display(),
        baseline.entries.len()
    );
    Ok(0)
}
