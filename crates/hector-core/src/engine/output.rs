//! Script-output parser.
//!
//! Recovers structure from script rules that emit canonical
//! `file:line:col: msg` output (ruff, eslint --format compact, clippy
//! --message-format short, …). Without it the whole stream lands verbatim in
//! `Violation.message` with `line: None, column: None`, and shapes like
//! `grep -nE 'pattern' {file}` produce violations like
//! `{ "line": null, "message": "14:matched-line" }` where the `14:` line
//! number pollutes the message.
//!
//! [`parse`] takes the chosen output stream (stdout *or* stderr — the
//! script engine still picks "stderr if non-empty else stdout") and
//! returns one [`ParsedRecord`] per detected hit. Modes are tried in
//! precedence order; the first that yields ≥1 record wins.
//!
//! 1. **JSON** — single object or array of objects with at least one of
//!    `message`/`msg`/`text` populated. Tolerant of extra fields. Optional
//!    `file`/`line`/`column` (or `col`) carry through.
//! 2. **Per-line `file:line:col: msg`** — the canonical clippy/ruff
//!    shape. Non-matching lines are ignored.
//! 3. **Per-line `file:line: msg`** — drop the column. Guarded: the first
//!    capture must contain a path separator (`/` or `\`) so we don't eat
//!    `<line>:<content>` from `grep -n` or `host:port: msg` log lines.
//! 4. **Per-line `line:msg`** — `grep -n` shape. `file` falls back to
//!    `known_file`.
//! 5. **Fallback** — one record with `line: None, column: None, message:
//!    raw.trim()`.
//!
//! The fallback never produces an empty record: empty input returns an
//! empty `Vec`, not `[ParsedRecord { message: "" }]`.

use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

/// One structured record extracted from a script's output.
///
/// `line`/`column` are `Option` because not every output mode supplies
/// them. `message` is always non-empty for non-fallback records (the
/// fallback uses `raw.trim()` and is suppressed when that's empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRecord {
    pub file: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
}

/// Parse `raw` into one or more [`ParsedRecord`]s.
///
/// `known_file` is the path the script was invoked against — used as the
/// `file` field for modes that don't supply one (the `line:msg` shape and
/// the fallback). It is *not* used to validate paths in the `file:line`
/// modes.
pub fn parse(raw: &str, known_file: &str) -> Vec<ParsedRecord> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    if let Some(records) = try_json(raw, known_file) {
        return records;
    }
    let canonical = parse_lines(raw, known_file, canonical_re(), parse_canonical_caps);
    if !canonical.is_empty() {
        return canonical;
    }
    let file_line = parse_lines(raw, known_file, file_line_re(), parse_file_line_caps);
    if !file_line.is_empty() {
        return file_line;
    }
    let line_msg = parse_lines(raw, known_file, line_msg_re(), parse_line_msg_caps);
    if !line_msg.is_empty() {
        return line_msg;
    }
    vec![ParsedRecord {
        file: known_file.to_string(),
        line: None,
        column: None,
        message: raw.trim().to_string(),
    }]
}

// --- regex globals --------------------------------------------------------

fn canonical_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Optional `[A-Za-z]:` prefix accommodates Windows drive paths
    // (`C:\foo.rs:14:5: msg`) which would otherwise be rejected by the
    // path capture's `[^:\s]+` (the bare-drive `:` would terminate it
    // mid-path). The capture is rejoined with capture 2 to form the file.
    RE.get_or_init(|| {
        Regex::new(r"^([A-Za-z]:)?([^:\s]+):(\d+):(\d+):\s*(.+)$")
            .expect("canonical regex compiles")
    })
}

fn file_line_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^([^:\s]+):(\d+):\s*(.+)$").expect("file:line regex compiles"))
}

fn line_msg_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(\d+):(.+)$").expect("line:msg regex compiles"))
}

// --- per-line dispatch ----------------------------------------------------

/// Walk `raw` line by line, applying `cap_to_record` to each captures hit
/// of `re`. Returns every record produced; the caller decides whether an
/// empty result means "fall through to the next mode."
fn parse_lines<F>(raw: &str, known_file: &str, re: &Regex, cap_to_record: F) -> Vec<ParsedRecord>
where
    F: Fn(&regex::Captures<'_>, &str) -> Option<ParsedRecord>,
{
    raw.lines()
        .filter_map(|line| {
            let caps = re.captures(line)?;
            cap_to_record(&caps, known_file)
        })
        .collect()
}

fn parse_canonical_caps(caps: &regex::Captures<'_>, _known_file: &str) -> Option<ParsedRecord> {
    // Capture 1 is the optional Windows drive prefix (`C:`); capture 2 is
    // the rest of the path. Concatenating preserves the full path on Linux
    // and macOS (where capture 1 is empty) and on Windows (where it's `X:`).
    let drive = caps.get(1).map(|m| m.as_str()).unwrap_or("");
    let rest = caps.get(2)?.as_str();
    let file = format!("{drive}{rest}");
    let line = caps.get(3)?.as_str().parse::<u32>().ok()?;
    let column = caps.get(4)?.as_str().parse::<u32>().ok()?;
    let message = caps.get(5)?.as_str().trim().to_string();
    Some(ParsedRecord {
        file,
        line: Some(line),
        column: Some(column),
        message,
    })
}

fn parse_file_line_caps(caps: &regex::Captures<'_>, _known_file: &str) -> Option<ParsedRecord> {
    let file = caps.get(1)?.as_str();
    if !looks_like_path(file) {
        return None;
    }
    let line = caps.get(2)?.as_str().parse::<u32>().ok()?;
    let message = caps.get(3)?.as_str().trim().to_string();
    Some(ParsedRecord {
        file: file.to_string(),
        line: Some(line),
        column: None,
        message,
    })
}

fn parse_line_msg_caps(caps: &regex::Captures<'_>, known_file: &str) -> Option<ParsedRecord> {
    let line = caps.get(1)?.as_str().parse::<u32>().ok()?;
    let message = caps.get(2)?.as_str().trim().to_string();
    Some(ParsedRecord {
        file: known_file.to_string(),
        line: Some(line),
        column: None,
        message,
    })
}

/// Heuristic guard on the `file:line: msg` mode: the first capture must
/// contain a path separator. Without this, `14:console.log('boom')` would
/// parse as `{ file: "14", line: <bad>, ... }` *and* `example.com:42: msg`
/// (a hostname:port followed by text) would parse as
/// `{ file: "example.com", line: 42 }` — a false-positive that hid the real
/// nature of the line.
///
/// Trade-off: `main.py:42: msg` (no column, no path separator) no longer
/// parses in this mode. It still parses correctly in `file_line_col` mode
/// when a column is present, and the `line:msg` mode plus the fallback
/// pick up the rest. Hostname false-positives were the more painful UX
/// regression to live with.
fn looks_like_path(s: &str) -> bool {
    s.contains('/') || s.contains('\\')
}

// --- JSON mode ------------------------------------------------------------

/// Try to interpret `raw` as JSON. Returns `Some(records)` if it parses
/// *and* contains at least one record with a recognized message field;
/// `None` otherwise (caller falls through to the line-oriented modes).
fn try_json(raw: &str, known_file: &str) -> Option<Vec<ParsedRecord>> {
    let value: Value = serde_json::from_str(raw.trim()).ok()?;
    let mut out = Vec::new();
    match &value {
        Value::Object(_) => {
            if let Some(rec) = json_object_to_record(&value, known_file) {
                out.push(rec);
            }
        }
        Value::Array(items) => {
            for item in items {
                if let Some(rec) = json_object_to_record(item, known_file) {
                    out.push(rec);
                }
            }
        }
        _ => return None,
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn json_object_to_record(value: &Value, known_file: &str) -> Option<ParsedRecord> {
    let obj = value.as_object()?;
    let message = ["message", "msg", "text"]
        .iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_str))?
        .to_string();
    let file = obj
        .get("file")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| known_file.to_string());
    let line = obj
        .get("line")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok());
    let column = obj
        .get("column")
        .or_else(|| obj.get("col"))
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok());
    Some(ParsedRecord {
        file,
        line,
        column,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: &str = "src/grep.txt";

    #[test]
    fn empty_input_returns_empty_vec() {
        assert_eq!(parse("", KNOWN), Vec::<ParsedRecord>::new());
    }

    #[test]
    fn whitespace_only_returns_empty_vec() {
        assert_eq!(parse("   \n\t \n", KNOWN), Vec::<ParsedRecord>::new());
    }

    #[test]
    fn json_object_with_full_fields() {
        let raw = r#"{"file":"src/foo.ts","line":14,"column":5,"message":"missing semicolon"}"#;
        let got = parse(raw, KNOWN);
        assert_eq!(
            got,
            vec![ParsedRecord {
                file: "src/foo.ts".into(),
                line: Some(14),
                column: Some(5),
                message: "missing semicolon".into(),
            }]
        );
    }

    #[test]
    fn json_object_msg_field_alias() {
        let raw = r#"{"line": 3, "msg": "via msg field"}"#;
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "via msg field");
        assert_eq!(got[0].file, KNOWN);
        assert_eq!(got[0].line, Some(3));
    }

    #[test]
    fn json_object_text_field_alias() {
        let raw = r#"{"text": "via text"}"#;
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "via text");
    }

    #[test]
    fn json_array_of_two_objects() {
        let raw = r#"[
            {"file":"a.rs","line":1,"column":2,"message":"first"},
            {"file":"b.rs","line":3,"column":4,"message":"second"}
        ]"#;
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].file, "a.rs");
        assert_eq!(got[0].line, Some(1));
        assert_eq!(got[1].file, "b.rs");
        assert_eq!(got[1].message, "second");
    }

    #[test]
    fn json_object_col_alias() {
        let raw = r#"{"line": 7, "col": 9, "message": "with col alias"}"#;
        let got = parse(raw, KNOWN);
        assert_eq!(got[0].column, Some(9));
    }

    #[test]
    fn json_with_no_message_field_falls_through_to_lines_then_fallback() {
        // No `message`/`msg`/`text` — JSON mode rejects, line modes don't
        // match (one big JSON-shaped string), so we land on the fallback.
        let raw = r#"{"file": "a.rs", "line": 1}"#;
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].line, None);
        assert_eq!(got[0].message, raw);
        assert_eq!(got[0].file, KNOWN);
    }

    #[test]
    fn json_scalar_falls_through_to_fallback() {
        // A plain JSON string isn't a record-shaped object.
        let raw = "\"just a string\"";
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, raw);
    }

    #[test]
    fn canonical_single_record() {
        let raw = "src/foo.ts:14:5: missing semicolon";
        let got = parse(raw, KNOWN);
        assert_eq!(
            got,
            vec![ParsedRecord {
                file: "src/foo.ts".into(),
                line: Some(14),
                column: Some(5),
                message: "missing semicolon".into(),
            }]
        );
    }

    #[test]
    fn canonical_two_records() {
        let raw = "src/foo.ts:14:5: missing semicolon\nsrc/bar.ts:21:3: unused import";
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].line, Some(14));
        assert_eq!(got[1].file, "src/bar.ts");
        assert_eq!(got[1].column, Some(3));
    }

    #[test]
    fn noise_line_then_canonical_returns_only_canonical() {
        let raw = "warning: something prefatory\nsrc/foo.ts:14:5: missing semicolon\n";
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].file, "src/foo.ts");
        assert_eq!(got[0].line, Some(14));
    }

    #[test]
    fn file_line_no_column() {
        let raw = "path/to/file.go:14: msg here";
        let got = parse(raw, KNOWN);
        assert_eq!(
            got,
            vec![ParsedRecord {
                file: "path/to/file.go".into(),
                line: Some(14),
                column: None,
                message: "msg here".into(),
            }]
        );
    }

    #[test]
    fn file_line_path_without_separator_falls_through() {
        // Tightening of `looks_like_path`: a bare `name.ext:line: msg` no
        // longer parses in `file:line: msg` mode (avoids `host.com:42: msg`
        // false-positives). The fallback still emits a sensible record.
        let raw = "main.py:42: complaint";
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].file, KNOWN);
        assert_eq!(got[0].line, None);
        assert_eq!(got[0].message, raw);
    }

    #[test]
    fn windows_drive_path_parses() {
        // The canonical regex must accept `C:\path:line:col: msg` even
        // though the drive letter introduces a `:` that would otherwise
        // terminate the path capture.
        let raw = r"C:\projects\foo.rs:14:5: missing semicolon";
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].file, r"C:\projects\foo.rs");
        assert_eq!(got[0].line, Some(14));
        assert_eq!(got[0].column, Some(5));
        assert_eq!(got[0].message, "missing semicolon");
    }

    #[test]
    fn hostname_port_does_not_parse_as_file_line() {
        // Without the `looks_like_path` tightening, `example.com:42: server
        // is down` would parse as `{ file: "example.com", line: 42 }` —
        // a false-positive that hid the line's true nature. Now it falls
        // through to `line:msg` mode (capture 1 must be all digits, so it
        // also rejects) and lands on the fallback.
        let raw = "example.com:42: server is down";
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 1);
        assert_ne!(got[0].file, "example.com");
        assert_eq!(got[0].file, KNOWN);
        assert_eq!(got[0].line, None);
        assert_eq!(got[0].message, raw);
    }

    #[test]
    fn grep_n_shape_uses_known_file() {
        // The bug: `grep -n 'pat' file` emits `<line>:<text>` and used to
        // land verbatim in the message. Now: line populates, message is
        // just the matched text, file is `known_file`.
        let raw = "14:console.log('boom')";
        let got = parse(raw, KNOWN);
        assert_eq!(
            got,
            vec![ParsedRecord {
                file: KNOWN.into(),
                line: Some(14),
                column: None,
                message: "console.log('boom')".into(),
            }]
        );
    }

    #[test]
    fn grep_n_two_lines() {
        let raw = "14:foo\n22:bar";
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].line, Some(14));
        assert_eq!(got[1].line, Some(22));
        assert_eq!(got[1].message, "bar");
    }

    #[test]
    fn freeform_text_falls_back() {
        let raw = "freeform error text";
        let got = parse(raw, KNOWN);
        assert_eq!(
            got,
            vec![ParsedRecord {
                file: KNOWN.into(),
                line: None,
                column: None,
                message: "freeform error text".into(),
            }]
        );
    }

    #[test]
    fn multiline_freeform_falls_back_trimmed() {
        let raw = "  first line\nsecond line  \n";
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].line, None);
        // `raw.trim()` collapses leading/trailing whitespace but preserves
        // the embedded newline — that's fine; the message is still legible.
        assert_eq!(got[0].message, "first line\nsecond line");
    }

    #[test]
    fn file_line_rejects_non_path_first_capture() {
        // Two-capture line where capture 1 doesn't look like a path. The
        // `file:line: msg` branch must reject so the `line:msg` (grep -n)
        // branch can still consider it. `nope:42: words` — `nope` has no
        // `/` `\` or `.`, so file_line says no; line_msg requires capture
        // 1 to be all digits, so it also says no; we land on fallback.
        let raw = "nope:42: words here";
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].line, None);
        assert_eq!(got[0].message, raw);
    }

    #[test]
    fn json_array_with_one_bad_object_keeps_the_good_one() {
        // The JSON parses, but one object lacks a message field. We keep
        // the well-formed sibling rather than dropping the whole array.
        let raw = r#"[
            {"file":"a.rs","line":1},
            {"file":"b.rs","line":3,"message":"good"}
        ]"#;
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "good");
    }

    #[test]
    fn json_array_all_bad_falls_through() {
        // Every element lacks a message. `try_json` returns `None`, so we
        // fall through. The line modes won't match a JSON blob, so the
        // fallback emits the raw text.
        let raw = r#"[{"file":"a.rs"},{"file":"b.rs"}]"#;
        let got = parse(raw, KNOWN);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, raw);
    }
}
