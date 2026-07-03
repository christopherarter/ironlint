use ironlint_core::diff::parse_unified;

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

// ChangedFile carries path + op; no line-number tracking. Confirm the parser
// yields the expected struct values. Both files in DIFF have `--- a/` +
// `+++ b/` headers, so they are classified as Modified.
#[test]
fn parse_unified_returns_only_path() {
    use ironlint_core::diff::parser::{ChangeOp, ChangedFile};
    use std::path::PathBuf;

    let files = parse_unified(DIFF).expect("parse");
    assert_eq!(files.len(), 2);
    assert_eq!(
        files[0],
        ChangedFile {
            path: PathBuf::from("src/foo.ts"),
            op: ChangeOp::Modified,
        }
    );
    assert_eq!(
        files[1],
        ChangedFile {
            path: PathBuf::from("src/bar.rs"),
            op: ChangeOp::Modified,
        }
    );
}

// Regression: the diff parser must reject path traversal in `+++ b/` headers.
// A malicious diff with `+++ b/../../../etc/passwd` would otherwise hand a path
// outside the workspace to the script or ast engines.
#[test]
fn parse_unified_rejects_path_traversal() {
    let diff = "--- a/foo\n+++ b/../../../etc/passwd\n@@ -0,0 +1 @@\n+x\n";
    let err = ironlint_core::diff::parser::parse_unified(diff)
        .expect_err("path traversal must be rejected");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("traversal") || msg.contains("absolute") || msg.contains(".."),
        "error should mention traversal; got: {msg}"
    );
}

// Regression: absolute paths leak through `+++ b//etc/passwd` because
// stripping the `+++ b/` prefix leaves `/etc/passwd`. Reject any leading `/`.
#[test]
fn parse_unified_rejects_absolute_path() {
    let diff = "--- a/foo\n+++ b//etc/passwd\n@@ -0,0 +1 @@\n+x\n";
    assert!(ironlint_core::diff::parser::parse_unified(diff).is_err());
}

// Regression: empty `+++ b/` path. After trim/strip this leaves `""`, which
// `starts_with('/')` and `components()` both pass — a downstream consumer that
// joins onto `cwd` would silently target `cwd` itself. Reject explicitly.
#[test]
fn parse_unified_rejects_empty_path() {
    let diff = "--- a/foo\n+++ b/\n@@ -0,0 +1 @@\n+x\n";
    let err = ironlint_core::diff::parser::parse_unified(diff)
        .expect_err("empty +++ b/ path must be rejected");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("empty"),
        "error should mention empty; got: {msg}"
    );
}

// Regression: CRLF diffs must not leave a trailing `\r` on the parsed path,
// which would mis-match globs (e.g. `src/**/*.py` vs `myfile.py\r`).
#[test]
fn parse_unified_trims_crlf_from_path() {
    let diff = "--- a/foo\r\n+++ b/myfile.py\r\n@@ -0,0 +1 @@\n+x\n";
    let files = ironlint_core::diff::parser::parse_unified(diff).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].path,
        std::path::PathBuf::from("myfile.py"),
        "trailing \\r must be stripped"
    );
}

/// POSIX `diff -u` headers include `\t<timestamp>` after the path. The parser
/// must strip that and yield a clean PathBuf.
#[test]
fn parse_unified_strips_tab_timestamp_from_path() {
    let input = "--- a/myfile.py\t2026-05-24 14:30:00 +0000\n\
                 +++ b/myfile.py\t2026-05-24 14:30:00 +0000\n\
                 @@ -1,1 +1,2 @@\n\
                  x\n\
                 +y\n";
    let files = ironlint_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::PathBuf::from("myfile.py"));
}

/// Paths without timestamps (the git case) must still parse.
#[test]
fn parse_unified_handles_path_without_timestamp() {
    let input = "--- a/x.rs\n+++ b/x.rs\n@@ -1,1 +1,2 @@\n a\n+b\n";
    let files = ironlint_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::PathBuf::from("x.rs"));
}

/// CRLF-terminated lines still strip cleanly when combined with a timestamp.
#[test]
fn parse_unified_handles_crlf_with_timestamp() {
    let input = "--- a/x.rs\t2026-05-24 14:30:00 +0000\r\n\
                 +++ b/x.rs\t2026-05-24 14:30:00 +0000\r\n\
                 @@ -1,1 +1,2 @@\r\n a\r\n+b\r\n";
    let files = ironlint_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::PathBuf::from("x.rs"));
}

// ChangeOp classification ----------------------------------------------------

/// `--- /dev/null` + `+++ b/<path>` is an addition.
#[test]
fn parse_unified_recognizes_addition() {
    use ironlint_core::diff::parser::ChangeOp;
    let input = "--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+fn a() {}\n";
    let files = ironlint_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::PathBuf::from("new.rs"));
    assert_eq!(files[0].op, ChangeOp::Added);
}

/// `--- a/<path>` + `+++ b/<path>` is a modification.
#[test]
fn parse_unified_recognizes_modification() {
    use ironlint_core::diff::parser::ChangeOp;
    let input = "--- a/existing.rs\n+++ b/existing.rs\n@@ -1 +1 @@\n-old\n+new\n";
    let files = ironlint_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::PathBuf::from("existing.rs"));
    assert_eq!(files[0].op, ChangeOp::Modified);
}

/// `--- a/<path>` + `+++ /dev/null` is a deletion. The parser must recognise
/// it and emit a `ChangedFile`, not drop the entry.
#[test]
fn parse_unified_recognizes_deletion() {
    use ironlint_core::diff::parser::ChangeOp;
    let input = "--- a/gone.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-fn a() {}\n-fn b() {}\n";
    let files = ironlint_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(
        files.len(),
        1,
        "deletion must produce exactly one ChangedFile"
    );
    assert_eq!(files[0].path, std::path::PathBuf::from("gone.rs"));
    assert_eq!(files[0].op, ChangeOp::Deleted);
}

/// Regression: deletion paths must also be path-validated. The `--- a/<path>`
/// segment of a deletion diff must run through `validate_path`, not just the
/// `+++ b/` arm — otherwise an unvalidated traversal path surfaces in the
/// public `ChangedFile` struct even though the runner skips deletions before
/// I/O.
#[test]
fn parse_unified_rejects_traversal_in_deletion_minus_path() {
    use ironlint_core::diff::parser::parse_unified;
    let result = parse_unified("--- a/../../etc/passwd\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-x\n");
    assert!(
        result.is_err(),
        "deletion with traversal path must be rejected at parse time"
    );
}

#[test]
fn parse_unified_rejects_absolute_in_deletion_minus_path() {
    use ironlint_core::diff::parser::parse_unified;
    let result = parse_unified("--- a//etc/passwd\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-x\n");
    assert!(
        result.is_err(),
        "deletion with absolute path must be rejected at parse time"
    );
}

/// A rename WITH content changes emits both `rename to` and a `---`/`+++`
/// pair. The parser must produce exactly ONE ChangedFile for the target
/// path (the more specific Modified entry), not a duplicate Renamed +
/// Modified pair. Regression test for the rename-with-content case.
#[test]
fn parse_unified_rename_with_content_yields_single_entry() {
    use ironlint_core::diff::parser::{ChangeOp, ChangedFile};
    use std::path::PathBuf;

    let input = "diff --git a/old.rs b/new.rs\n\
        similarity index 80%\n\
        rename from old.rs\n\
        rename to new.rs\n\
        --- a/old.rs\n\
        +++ b/new.rs\n\
        @@ -1,1 +1,1 @@\n\
        -old\n\
        +new\n";
    let files = ironlint_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(
        files.len(),
        1,
        "rename-with-content must yield exactly one entry, got {files:?}"
    );
    assert_eq!(
        files[0],
        ChangedFile {
            path: PathBuf::from("new.rs"),
            op: ChangeOp::Modified,
        }
    );
}

/// A pure `git mv` diff (no ---/+++ pair, only rename from/to headers)
/// must surface the renamed file in the changed set.
#[test]
fn parse_unified_recognizes_rename() {
    use ironlint_core::diff::parser::{ChangeOp, ChangedFile};
    use std::path::PathBuf;

    let input = "diff --git a/old.rs b/new.rs\n\
        similarity index 100%\n\
        rename from old.rs\n\
        rename to new.rs\n";
    let files = ironlint_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0],
        ChangedFile {
            path: PathBuf::from("new.rs"),
            op: ChangeOp::Renamed,
        }
    );
}

/// Paths are C-quoted by git when core.quotePath=true and the path
/// contains non-ASCII bytes. The parser must unquote them.
#[test]
fn parse_unified_unquotes_c_quoted_path() {
    let input = "--- \"a/caf\\303\\251.rs\"\n+++ \"b/caf\\303\\251.rs\"\n@@ -1,1 +1,1 @@\n-a\n+b\n";
    let files = ironlint_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::PathBuf::from("café.rs"));
}

/// An unrecognized +++ header must fail closed (parse error), not be
/// silently dropped, so a changed file cannot bypass the gate.
#[test]
fn parse_unified_rejects_unrecognized_plus_plus_plus_header() {
    let input = "--- a/foo.rs\n+++ c/foo.rs\n@@ -1,1 +1,1 @@\n-a\n+b\n";
    let err = ironlint_core::diff::parser::parse_unified(input)
        .expect_err("unrecognized +++ header must be a hard parse error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("+++") || msg.contains("unrecognized"),
        "error should mention the malformed +++ header; got: {msg}"
    );
}

// Quoted-path edge cases -----------------------------------------------------

/// C-quoted paths may contain standard escapes. Each must decode to the real
/// byte in the parsed path.
#[test]
fn parse_unified_unquotes_standard_c_escapes() {
    let cases = [
        (
            r#"--- "a/weird\\name.rs"
+++ "b/weird\\name.rs"
@@ -1,1 +1,1 @@
-a
+b
"#,
            "weird\\name.rs",
        ),
        (
            r#"--- "a/quote\".rs"
+++ "b/quote\".rs"
@@ -1,1 +1,1 @@
-a
+b
"#,
            "quote\".rs",
        ),
        (
            r#"--- "a/weird\nname.rs"
+++ "b/weird\nname.rs"
@@ -1,1 +1,1 @@
-a
+b
"#,
            "weird\nname.rs",
        ),
        (
            r#"--- "a/weird\rname.rs"
+++ "b/weird\rname.rs"
@@ -1,1 +1,1 @@
-a
+b
"#,
            "weird\rname.rs",
        ),
        (
            r#"--- "a/weird\tname.rs"
+++ "b/weird\tname.rs"
@@ -1,1 +1,1 @@
-a
+b
"#,
            "weird\tname.rs",
        ),
    ];
    for (input, expected) in cases {
        let files = ironlint_core::diff::parser::parse_unified(input).expect("parses");
        assert_eq!(files.len(), 1, "input: {input:?}");
        assert_eq!(
            files[0].path,
            std::path::PathBuf::from(expected),
            "input: {input:?}"
        );
    }
}

/// A truncated escape at the end of a quoted path must be a hard parse error.
#[test]
fn parse_unified_rejects_truncated_escape_in_quoted_path() {
    let input = r#"--- "a/bad\"
+++ "b/good.rs"
@@ -1,1 +1,1 @@
-a
+b
"#;
    let err = ironlint_core::diff::parser::parse_unified(input)
        .expect_err("truncated escape must be a hard parse error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("truncated"),
        "error should mention truncated escape; got: {msg}"
    );
}

/// Non-octal digits in an escape sequence must be rejected.
#[test]
fn parse_unified_rejects_invalid_octal_escape_in_quoted_path() {
    let input = r#"--- "a/bad\8.rs"
+++ "b/bad\8.rs"
@@ -1,1 +1,1 @@
-a
+b
"#;
    let err = ironlint_core::diff::parser::parse_unified(input)
        .expect_err("invalid octal escape must be a hard parse error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("octal") || msg.contains("escape"),
        "error should mention octal/escape; got: {msg}"
    );
}

/// An octal sequence that does not form valid UTF-8 must be rejected.
#[test]
fn parse_unified_rejects_invalid_utf8_from_octal_escape() {
    let input = r#"--- "a/\377.rs"
+++ "b/\377.rs"
@@ -1,1 +1,1 @@
-a
+b
"#;
    let err = ironlint_core::diff::parser::parse_unified(input)
        .expect_err("invalid UTF-8 from octal escape must be a hard parse error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("UTF-8") || msg.contains("utf-8"),
        "error should mention UTF-8; got: {msg}"
    );
}

/// Quoted paths may still carry the POSIX timestamp suffix; splitting on the
/// real tab must happen before unquoting so escaped tabs inside the path are
/// not mistaken for the timestamp separator.
#[test]
fn parse_unified_unquotes_c_quoted_path_with_timestamp() {
    let input = r#"--- "a/caf\303\251.rs"	2024-01-01 12:00:00 +0000
+++ "b/caf\303\251.rs"	2024-01-01 12:00:00 +0000
@@ -1,1 +1,1 @@
-a
+b
"#;
    let files = ironlint_core::diff::parser::parse_unified(input).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::PathBuf::from("café.rs"));
}

/// An unsupported C escape (e.g. `\x`) inside a quoted git path must be a
/// hard parse error, not silently passed through or dropped.
#[test]
fn parse_unified_rejects_unsupported_escape_in_quoted_path() {
    let input = "--- \"a/bad\\x.rs\"\n+++ \"a/bad\\x.rs\"\n@@ -1,1 +1,1 @@\n-a\n+b\n";
    let err = ironlint_core::diff::parser::parse_unified(input)
        .expect_err("unsupported escape in quoted path must be a hard parse error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("unsupported escape"),
        "error should mention unsupported escape; got: {msg}"
    );
}
