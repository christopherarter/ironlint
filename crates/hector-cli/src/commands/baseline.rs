use anyhow::Result;
use hector_core::baseline::Baseline;
use hector_core::runner::{CheckInput, HectorEngine};
use std::path::Path;

pub fn run(config: &Path, scan_glob: Option<String>) -> Result<i32> {
    let engine = HectorEngine::load(config)?;
    let dir = config.parent().unwrap_or(Path::new("."));
    let baseline_path = dir.join(".hector/baseline.json");
    let mut baseline = Baseline::load(&baseline_path)?;

    let pattern = scan_glob.unwrap_or_else(|| "**/*".to_string());
    let glob = globset::Glob::new(&pattern)?.compile_matcher();

    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(dir).unwrap_or(path);
        if !glob.is_match(rel) {
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let verdict = engine.check(CheckInput::File {
            path: path.to_path_buf(),
            content,
        })?;
        for v in verdict.violations {
            baseline.add(&v);
        }
    }
    baseline.save(&baseline_path)?;
    println!(
        "baseline written: {} ({} entries)",
        baseline_path.display(),
        baseline.fingerprints.len()
    );
    Ok(0)
}
