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
