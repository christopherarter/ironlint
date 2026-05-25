use hector_core::diff::parse_unified;

const DIFF: &str = "\
--- a/src/foo.ts
+++ b/src/foo.ts
@@ -1,3 +1,4 @@
 line one
-old line
+new line
+added line
 line three
--- a/src/bar.rs
+++ b/src/bar.rs
@@ -10,2 +10,3 @@
 keep
+added
 keep
";

#[test]
fn parses_two_files() {
    let files = parse_unified(DIFF).expect("parse");
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path.to_str().unwrap(), "src/foo.ts");
    assert_eq!(files[1].path.to_str().unwrap(), "src/bar.rs");
}

#[test]
fn captures_added_line_numbers() {
    let files = parse_unified(DIFF).unwrap();
    let foo = &files[0];
    assert_eq!(foo.added_lines, vec![2, 3]);
    let bar = &files[1];
    assert_eq!(bar.added_lines, vec![11]);
}

// Regression: P0-4 — diff parser must reject path traversal in `+++ b/` headers.
// A malicious diff with `+++ b/../../../etc/passwd` would otherwise hand a path
// outside the workspace to the semantic context-reader or script engines.
#[test]
fn parse_unified_rejects_path_traversal() {
    let diff = "--- a/foo\n+++ b/../../../etc/passwd\n@@ -0,0 +1 @@\n+x\n";
    let err = hector_core::diff::parser::parse_unified(diff)
        .expect_err("path traversal must be rejected");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("traversal") || msg.contains("absolute") || msg.contains(".."),
        "error should mention traversal; got: {msg}"
    );
}

// Regression: P0-4 — absolute paths leak through `+++ b//etc/passwd` because
// stripping the `+++ b/` prefix leaves `/etc/passwd`. Reject any leading `/`.
#[test]
fn parse_unified_rejects_absolute_path() {
    let diff = "--- a/foo\n+++ b//etc/passwd\n@@ -0,0 +1 @@\n+x\n";
    assert!(hector_core::diff::parser::parse_unified(diff).is_err());
}

// Regression: empty `+++ b/` path. After trim/strip this leaves `""`, which
// `starts_with('/')` and `components()` both pass — a downstream consumer that
// joins onto `cwd` would silently target `cwd` itself. Reject explicitly.
#[test]
fn parse_unified_rejects_empty_path() {
    let diff = "--- a/foo\n+++ b/\n@@ -0,0 +1 @@\n+x\n";
    let err = hector_core::diff::parser::parse_unified(diff)
        .expect_err("empty +++ b/ path must be rejected");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("empty"),
        "error should mention empty; got: {msg}"
    );
}

// A `+++ ...` line inside a hunk (not the file header `+++ b/`) is skipped
// without counting toward added lines. Without this case, the branch
// `!raw.starts_with("+++")` going false stays uncovered.
#[test]
fn parse_unified_ignores_literal_triple_plus_inside_hunk() {
    let diff = "\
--- a/notes.md
+++ b/notes.md
@@ -1,2 +1,3 @@
 head
+added
+++ literal triple plus in content
";
    let files = parse_unified(diff).unwrap();
    assert_eq!(
        files.len(),
        1,
        "the `+++` content line must not start a new file"
    );
    assert_eq!(
        files[0].added_lines,
        vec![2],
        "only the real `+added` line counts; the `+++` content line is skipped"
    );
}

// Regression: P2-10 — CRLF diffs left a trailing `\r` on the parsed path,
// which silently mis-matched globs (e.g. `src/**/*.py` vs `myfile.py\r`).
#[test]
fn parse_unified_trims_crlf_from_path() {
    let diff = "--- a/foo\r\n+++ b/myfile.py\r\n@@ -0,0 +1 @@\n+x\n";
    let files = hector_core::diff::parser::parse_unified(diff).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].path,
        std::path::PathBuf::from("myfile.py"),
        "trailing \\r must be stripped"
    );
}

/// A2 regression: POSIX `diff -u` headers include `\t<timestamp>` after
/// the path. The parser must strip that and yield a clean PathBuf.
#[test]
fn parse_unified_strips_tab_timestamp_from_path() {
    let input = "--- a/myfile.py\t2026-05-24 14:30:00 +0000\n\
                 +++ b/myfile.py\t2026-05-24 14:30:00 +0000\n\
                 @@ -1,1 +1,2 @@\n\
                  x\n\
                 +y\n";
    let files = hector_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::PathBuf::from("myfile.py"));
}

/// A2: paths without timestamps (the git case) must still parse.
#[test]
fn parse_unified_handles_path_without_timestamp() {
    let input = "--- a/x.rs\n+++ b/x.rs\n@@ -1,1 +1,2 @@\n a\n+b\n";
    let files = hector_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::PathBuf::from("x.rs"));
}

/// A2: CRLF-terminated lines still strip cleanly when combined with a timestamp.
#[test]
fn parse_unified_handles_crlf_with_timestamp() {
    let input = "--- a/x.rs\t2026-05-24 14:30:00 +0000\r\n\
                 +++ b/x.rs\t2026-05-24 14:30:00 +0000\r\n\
                 @@ -1,1 +1,2 @@\r\n a\r\n+b\r\n";
    let files = hector_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::PathBuf::from("x.rs"));
}
