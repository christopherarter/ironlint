use hector_core::runner::{CheckOptions, HectorEngine};
use std::path::PathBuf;
use tempfile::tempdir;

/// An absolute path that doesn't exist on disk fails canonicalize, so
/// resolve_input_path returns the original absolute path unchanged —
/// diff-mode callers may reference files not yet on disk.
#[test]
fn resolve_input_path_returns_absolute_unchanged_when_not_on_disk() {
    let tmp = tempdir().unwrap();
    let config = write_trusted_minimal_config(tmp.path());
    let engine = HectorEngine::load(&config).expect("load");
    let abs = PathBuf::from("/some/absolute/path.rs");
    // File doesn't exist → canonicalize fails → early-return Ok(resolved).
    let resolved = engine.resolve_input_path(&abs).unwrap();
    assert_eq!(resolved, abs);
}

/// A relative path is joined onto config_dir. The joined path doesn't exist
/// on disk here, so canonicalize fails and we get the raw joined path back.
#[test]
fn resolve_input_path_joins_relative_onto_config_dir() {
    let tmp = tempdir().unwrap();
    let config = write_trusted_minimal_config(tmp.path());
    let engine = HectorEngine::load(&config).expect("load");
    let rel = PathBuf::from("src/lib.rs");
    let resolved = engine.resolve_input_path(&rel).unwrap();
    assert_eq!(resolved, tmp.path().join("src/lib.rs"));
}

/// A file that exists inside config_dir is accepted (no error).
#[test]
fn resolve_input_path_accepts_file_inside_config_dir() {
    let tmp = tempdir().unwrap();
    let config = write_trusted_minimal_config(tmp.path());
    let engine = HectorEngine::load(&config).expect("load");
    // Write an actual file inside the config dir so canonicalize succeeds.
    let inside = tmp.path().join("src").join("lib.rs");
    std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
    std::fs::write(&inside, "fn main() {}").unwrap();
    let resolved = engine.resolve_input_path(&inside).unwrap();
    assert_eq!(resolved, inside.canonicalize().unwrap());
}

/// A file that exists outside config_dir is rejected by default.
#[test]
fn resolve_input_path_rejects_external_path_by_default() {
    let tmp = tempdir().unwrap();
    let config = write_trusted_minimal_config(tmp.path());
    let engine = HectorEngine::load(&config).expect("load");
    // Write a file in a *different* temp dir (guaranteed external).
    let external_dir = tempdir().unwrap();
    let external = external_dir.path().join("target.rs");
    std::fs::write(&external, "x").unwrap();
    let result = engine.resolve_input_path(&external);
    assert!(result.is_err(), "expected Err for external path");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("outside") || msg.contains("external"),
        "error must mention 'outside' or 'external', got: {msg}"
    );
}

/// With allow_external_paths=true, a file outside config_dir is accepted.
#[test]
fn resolve_input_path_allows_external_path_when_opted_in() {
    let tmp = tempdir().unwrap();
    let config = write_trusted_minimal_config(tmp.path());
    let options = CheckOptions {
        allow_external_paths: true,
        ..CheckOptions::default()
    };
    let engine = HectorEngine::builder()
        .with_options(options)
        .load(&config)
        .expect("load");
    let external_dir = tempdir().unwrap();
    let external = external_dir.path().join("target.rs");
    std::fs::write(&external, "x").unwrap();
    let resolved = engine.resolve_input_path(&external).unwrap();
    assert_eq!(resolved, external.canonicalize().unwrap());
}

/// Performance regression: rule_matches_path must not rebuild a GlobSet on
/// every call. 10,000 matches against a single cached ScopeMatcher must
/// complete well under 100 ms; rebuilding per call blows past that threshold.
#[test]
fn rule_matches_path_does_not_rebuild_matcher() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_trusted_minimal_config(tmp.path());
    let engine = HectorEngine::load(&config).expect("load");
    let rule_id = "r";
    let path = std::path::PathBuf::from("src/lib.rs");
    let start = std::time::Instant::now();
    for _ in 0..10_000 {
        let _ = engine.rule_matches_path(rule_id, &path);
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "10_000 matches took {elapsed:?} — likely rebuilding ScopeMatcher per call"
    );
}

fn write_trusted_minimal_config(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join(".hector.yml");
    let yaml = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n";
    std::fs::write(&path, yaml).unwrap();
    let signed = hector_core::trust::write_trust_block(yaml).unwrap();
    std::fs::write(&path, signed).unwrap();
    path
}
