use std::path::Path;

const CONTEXT_LINES: usize = 3;

struct ChangedSpan {
    prefix: usize,
    old_end: usize,
    new_end: usize,
}

struct HunkWindow {
    start: usize,
    old_end: usize,
    new_end: usize,
}

/// Synthesize a single-file unified diff from current on-disk content to
/// proposed content.
///
/// The diff is line-oriented and intentionally conservative: it finds the
/// shared prefix/suffix and emits one hunk around the changed middle, which
/// is enough evidence for semantic rules in pre-write checks without adding a
/// diffing dependency.
pub fn synthesize_unified(path: &Path, old: Option<&str>, new: &str) -> String {
    let old_body = old.unwrap_or_default();
    if old.is_some() && old_body == new {
        return String::new();
    }
    if old.is_none() && new.is_empty() {
        return String::new();
    }

    let old_lines = split_lines(old_body);
    let new_lines = split_lines(new);
    let span = changed_span(&old_lines, &new_lines);
    let window = hunk_window(&old_lines, &new_lines, &span);
    render_diff(path, old.is_none(), &old_lines, &new_lines, &span, &window)
}

fn split_lines(s: &str) -> Vec<&str> {
    if s.is_empty() {
        Vec::new()
    } else {
        s.lines().collect()
    }
}

fn changed_span(old: &[&str], new: &[&str]) -> ChangedSpan {
    let prefix = common_prefix_len(old, new);
    let suffix = common_suffix_len(old, new, prefix);
    ChangedSpan {
        prefix,
        old_end: old.len().saturating_sub(suffix),
        new_end: new.len().saturating_sub(suffix),
    }
}

fn common_prefix_len(old: &[&str], new: &[&str]) -> usize {
    old.iter()
        .zip(new.iter())
        .take_while(|(a, b)| a == b)
        .count()
}

fn common_suffix_len(old: &[&str], new: &[&str], prefix: usize) -> usize {
    let max_suffix = old.len().min(new.len()).saturating_sub(prefix);
    (0..max_suffix)
        .take_while(|i| old[old.len() - 1 - i] == new[new.len() - 1 - i])
        .count()
}

fn hunk_window(old: &[&str], new: &[&str], span: &ChangedSpan) -> HunkWindow {
    HunkWindow {
        start: span.prefix.saturating_sub(CONTEXT_LINES),
        old_end: old.len().min(span.old_end + CONTEXT_LINES),
        new_end: new.len().min(span.new_end + CONTEXT_LINES),
    }
}

fn render_diff(
    path: &Path,
    old_missing: bool,
    old: &[&str],
    new: &[&str],
    span: &ChangedSpan,
    window: &HunkWindow,
) -> String {
    let mut out = String::new();
    push_headers(&mut out, path, old_missing);
    push_hunk_header(&mut out, window);
    push_context(&mut out, &old[window.start..span.prefix]);
    push_changed(&mut out, '-', &old[span.prefix..span.old_end]);
    push_changed(&mut out, '+', &new[span.prefix..span.new_end]);
    push_context(&mut out, &old[span.old_end..window.old_end]);
    out
}

fn push_headers(out: &mut String, path: &Path, old_missing: bool) {
    let path = diff_path(path);
    if old_missing {
        out.push_str("--- /dev/null\n");
    } else {
        out.push_str("--- a/");
        out.push_str(&path);
        out.push('\n');
    }
    out.push_str("+++ b/");
    out.push_str(&path);
    out.push('\n');
}

fn push_hunk_header(out: &mut String, window: &HunkWindow) {
    let old_count = window.old_end.saturating_sub(window.start);
    let new_count = window.new_end.saturating_sub(window.start);
    out.push_str("@@ -");
    out.push_str(&range_part(window.start, old_count));
    out.push_str(" +");
    out.push_str(&range_part(window.start, new_count));
    out.push_str(" @@\n");
}

fn range_part(zero_based_start: usize, count: usize) -> String {
    if count == 0 {
        "0,0".to_string()
    } else if count == 1 {
        (zero_based_start + 1).to_string()
    } else {
        format!("{},{}", zero_based_start + 1, count)
    }
}

fn push_context(out: &mut String, lines: &[&str]) {
    push_changed(out, ' ', lines);
}

fn push_changed(out: &mut String, prefix: char, lines: &[&str]) {
    for line in lines {
        push_diff_line(out, prefix, line);
    }
}

fn push_diff_line(out: &mut String, prefix: char, line: &str) {
    let rendered = format!("{prefix}{line}");
    if looks_like_diff_header(&rendered) {
        out.push('\\');
    }
    out.push_str(&rendered);
    out.push('\n');
}

fn looks_like_diff_header(line: &str) -> bool {
    line.starts_with("--- ") || line.starts_with("+++ ") || line.starts_with("@@ ")
}

fn diff_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches('/')
        .chars()
        .map(|c| match c {
            '\n' | '\r' | '\t' => '_',
            _ => c,
        })
        .collect()
}
