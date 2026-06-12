use hector_core::diff::synthesize_unified;
use std::path::Path;

#[test]
fn unchanged_existing_file_returns_empty_diff() {
    let diff = synthesize_unified(
        Path::new("src/app.ts"),
        Some("const x = 1;\n"),
        "const x = 1;\n",
    );
    assert!(diff.is_empty());
}

#[test]
fn missing_empty_file_returns_empty_diff() {
    let diff = synthesize_unified(Path::new("src/app.ts"), None, "");
    assert!(diff.is_empty());
}

#[test]
fn new_file_uses_dev_null_header() {
    let diff = synthesize_unified(
        Path::new("src/app.ts"),
        None,
        "const x = 1;\nconst y = 2;\n",
    );
    assert!(diff.starts_with("--- /dev/null\n+++ b/src/app.ts\n"));
    assert!(diff.contains("@@ -0,0 +1,2 @@"));
    assert!(diff.contains("+const x = 1;\n+const y = 2;\n"));
}

#[test]
fn one_line_replacement_uses_compact_range_header() {
    let diff = synthesize_unified(Path::new("src/app.ts"), Some("old\n"), "new\n");
    assert!(diff.contains("@@ -1 +1 @@"));
    assert!(diff.contains("-old\n+new\n"));
}

#[test]
fn replacement_keeps_nearby_context() {
    let old = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
    let new = "one\ntwo\nTHREE\nfour\nfive\nsix\nseven\n";
    let diff = synthesize_unified(Path::new("src/app.ts"), Some(old), new);
    assert!(diff.contains(" one\n two\n-three\n+THREE\n four\n five\n six\n"));
    assert!(!diff.contains(" seven\n"));
}

#[test]
fn header_like_added_lines_are_escaped() {
    let diff = synthesize_unified(Path::new("src/app.ts"), None, "++ b/other.ts\n");
    assert!(diff.contains("\\+++ b/other.ts\n"));
}

#[test]
fn path_control_characters_are_sanitized() {
    let diff = synthesize_unified(Path::new("src/\napp\t.ts"), None, "x\n");
    assert!(diff.starts_with("--- /dev/null\n+++ b/src/_app_.ts\n"));
}
