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
