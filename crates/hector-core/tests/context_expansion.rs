use hector_core::config::ContextScope;
use hector_core::engine::context::expand_context;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn diff_scope_returns_diff_as_is() {
    let result = expand_context(
        ContextScope::Diff,
        Some("--- a/foo\n+++ b/foo\n@@ -1 +1 @@\n-old\n+new"),
        None,
        None,
        Path::new("/tmp"),
    );
    let (primary, ctx) = result.unwrap();
    assert!(primary.contains("+new"));
    assert!(ctx.is_none());
}

#[test]
fn file_scope_returns_file_content() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "the whole file\n").unwrap();
    let result = expand_context(ContextScope::File, None, Some(&file), None, dir.path());
    let (primary, ctx) = result.unwrap();
    assert!(primary.contains("the whole file"));
    assert!(ctx.is_none());
}

#[test]
fn repo_scope_falls_back_to_file_for_now() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "file content\n").unwrap();
    let result = expand_context(ContextScope::Repo, None, Some(&file), None, dir.path());
    let (primary, ctx) = result.unwrap();
    assert!(primary.contains("file content"));
    // Repo expansion is degraded in 0.1b — returns file content with a note in ctx.
    assert!(ctx.is_some());
}

#[test]
fn diff_scope_errors_when_diff_is_missing() {
    let err = expand_context(ContextScope::Diff, None, None, None, Path::new("/tmp"))
        .expect_err("missing diff");
    assert!(format!("{err:#}").contains("diff"));
}

#[test]
fn file_scope_errors_when_file_is_missing() {
    let err = expand_context(ContextScope::File, None, None, None, Path::new("/tmp"))
        .expect_err("missing file anchor");
    assert!(format!("{err:#}").contains("file"));
}

#[test]
fn repo_scope_errors_when_file_anchor_is_missing() {
    // Repo shares File's resolution: with neither authoritative content nor a
    // disk anchor there is nothing to read, so it errors. The message is the
    // unified file-resolution error ("no file or content provided").
    let err = expand_context(ContextScope::Repo, None, None, None, Path::new("/tmp"))
        .expect_err("missing repo anchor");
    assert!(format!("{err:#}").contains("no file or content"));
}

#[test]
fn file_scope_surfaces_read_error_for_nonexistent_path() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("nope.txt");
    let result = expand_context(ContextScope::File, None, Some(&missing), None, dir.path());
    assert!(result.is_err());
}

#[test]
fn repo_scope_surfaces_read_error_for_nonexistent_path() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("nope.txt");
    let result = expand_context(ContextScope::Repo, None, Some(&missing), None, dir.path());
    assert!(result.is_err());
}

#[test]
fn file_scope_prefers_authoritative_content_over_disk() {
    // PreToolUse passes proposed content that is not yet on disk. When the
    // caller supplies authoritative content, File scope must use it, not the
    // stale on-disk bytes.
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "OLD DISK CONTENT\n").unwrap();
    let (primary, ctx) = expand_context(
        ContextScope::File,
        None,
        Some(&file),
        Some("PROPOSED NEW CONTENT"),
        dir.path(),
    )
    .unwrap();
    assert!(primary.contains("PROPOSED NEW CONTENT"));
    assert!(!primary.contains("OLD DISK CONTENT"));
    assert!(ctx.is_none());
}

#[test]
fn file_scope_uses_supplied_content_even_when_file_is_absent() {
    // A brand-new file proposed in PreToolUse mode has no disk bytes at all.
    let dir = tempdir().unwrap();
    let missing = dir.path().join("brand-new.txt");
    let (primary, _ctx) = expand_context(
        ContextScope::File,
        None,
        Some(&missing),
        Some("content for a file not yet written"),
        dir.path(),
    )
    .expect("supplied content needs no disk read");
    assert!(primary.contains("content for a file not yet written"));
}

#[test]
fn file_scope_falls_back_to_disk_when_no_content_supplied() {
    // Diff mode and the prompt-preview path pass content: None and must keep
    // reading from disk.
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "the whole file\n").unwrap();
    let (primary, _ctx) =
        expand_context(ContextScope::File, None, Some(&file), None, dir.path()).unwrap();
    assert!(primary.contains("the whole file"));
}
