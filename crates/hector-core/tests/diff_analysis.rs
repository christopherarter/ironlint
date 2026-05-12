use hector_core::diff::analysis::{can_match_diff, CanMatch, SkipReason};
use std::path::Path;

const ADD_HELLO_RS: &str = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,1 +1,2 @@
 fn main() {}
+fn hello() {}
";

const WHITESPACE_ONLY_RS: &str = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,4 @@
 fn main() {}
+
+    
 fn other() {}
";

const COMMENT_ONLY_RS: &str = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,1 +1,3 @@
 fn main() {}
+// new comment
+/* block */
";

const COMMENT_ONLY_PY: &str = "\
--- a/app.py
+++ b/app.py
@@ -1,1 +1,2 @@
 x = 1
+# pep8 comment
";

const PURE_DELETION: &str = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,1 @@
 fn main() {}
-fn dead() {}
";

const EMPTY_DIFF: &str = "";

#[test]
fn empty_diff_is_skipped() {
    let r = can_match_diff(
        EMPTY_DIFF,
        Path::new("src/lib.rs"),
        "no panic in library code",
    );
    assert!(matches!(r, CanMatch::No(SkipReason::Empty)));
}

#[test]
fn whitespace_only_diff_is_skipped() {
    let r = can_match_diff(
        WHITESPACE_ONLY_RS,
        Path::new("src/lib.rs"),
        "no panic in library code",
    );
    assert!(matches!(r, CanMatch::No(SkipReason::WhitespaceOnly)));
}

#[test]
fn comment_only_rs_diff_is_skipped_when_rule_not_about_comments() {
    let r = can_match_diff(
        COMMENT_ONLY_RS,
        Path::new("src/lib.rs"),
        "no unwrap in library code",
    );
    assert!(matches!(r, CanMatch::No(SkipReason::CommentsOnly)));
}

#[test]
fn comment_only_py_diff_is_skipped() {
    let r = can_match_diff(COMMENT_ONLY_PY, Path::new("app.py"), "no print statements");
    assert!(matches!(r, CanMatch::No(SkipReason::CommentsOnly)));
}

#[test]
fn comment_only_diff_is_not_skipped_when_rule_mentions_comments() {
    let r = can_match_diff(
        COMMENT_ONLY_RS,
        Path::new("src/lib.rs"),
        "no TODO comments left behind",
    );
    assert!(matches!(r, CanMatch::Yes));
}

#[test]
fn pure_deletion_skipped_for_avoid_rule() {
    let r = can_match_diff(
        PURE_DELETION,
        Path::new("src/lib.rs"),
        "avoid panics in lib code",
    );
    assert!(matches!(r, CanMatch::No(SkipReason::PureDeletion)));
}

#[test]
fn pure_deletion_skipped_for_no_x_rule() {
    let r = can_match_diff(
        PURE_DELETION,
        Path::new("src/lib.rs"),
        "no eprintln! in lib code",
    );
    assert!(matches!(r, CanMatch::No(SkipReason::PureDeletion)));
}

#[test]
fn pure_deletion_skipped_for_dont_x_rule() {
    let r = can_match_diff(PURE_DELETION, Path::new("src/lib.rs"), "don't call unwrap");
    assert!(matches!(r, CanMatch::No(SkipReason::PureDeletion)));
}

#[test]
fn pure_deletion_dispatched_for_positive_rule() {
    let r = can_match_diff(
        PURE_DELETION,
        Path::new("src/lib.rs"),
        "functions must have docs",
    );
    assert!(matches!(r, CanMatch::Yes));
}

#[test]
fn real_addition_diff_is_dispatched() {
    let r = can_match_diff(
        ADD_HELLO_RS,
        Path::new("src/lib.rs"),
        "no useEffect derives state",
    );
    assert!(matches!(r, CanMatch::Yes));
}

#[test]
fn unknown_extension_with_comment_chars_is_dispatched() {
    let diff = "\
--- a/foo.xyz
+++ b/foo.xyz
@@ -1,1 +1,2 @@
 a
+// looks like a comment but we don't know this language
";
    let r = can_match_diff(diff, Path::new("foo.xyz"), "no foo");
    assert!(matches!(r, CanMatch::Yes));
}

#[test]
fn mixed_comment_and_code_is_dispatched() {
    let diff = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,1 +1,3 @@
 fn main() {}
+// helper
+fn hello() {}
";
    let r = can_match_diff(diff, Path::new("src/lib.rs"), "no helpers");
    assert!(matches!(r, CanMatch::Yes));
}
