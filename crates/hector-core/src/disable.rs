//! `hector-disable: <gate-id>` line directives.
//!
//! A directive anywhere in a checked file's proposed content suppresses that
//! gate for that file (one gate per directive; the id list ends at whitespace,
//! `*`, or a comment terminator `//`/`*/`). The matcher is id-agnostic.

/// True if any `hector-disable: <gate-id>` directive in `content` names
/// `gate_id`. File-wide: gates produce one verdict per file, so a directive
/// anywhere in the file suppresses the gate.
pub fn is_disabled(content: &str, gate_id: &str) -> bool {
    content.lines().any(|line| {
        parse_disable_directives(line)
            .iter()
            .any(|id| id == gate_id)
    })
}

fn parse_disable_directives(line: &str) -> Vec<String> {
    let marker = "hector-disable:";
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(idx) = rest.find(marker) {
        let after = &rest[idx + marker.len()..];
        let (ids, consumed) = collect_gate_ids(after);
        out.extend(ids);
        rest = &after[consumed..];
    }
    out
}

/// Collect comma- or whitespace-separated gate IDs after `hector-disable:`.
/// Stops at end-of-input, the literal token `reason:`, a bare `*`, or a `/`
/// that begins a comment terminator (`//` or `/*`).
///
/// A `/` only terminates when the next byte is `/` or `*` — treating `/` as an
/// unconditional terminator would truncate namespaced gate IDs like
/// `python/no-print` to `python`.
fn collect_gate_ids(s: &str) -> (Vec<String>, usize) {
    let mut ids = Vec::new();
    let mut i = 0;
    let bytes = s.as_bytes();
    let slash_terminates_at = |buf: &[u8], idx: usize| -> bool {
        buf.get(idx).copied() == Some(b'/')
            && matches!(buf.get(idx + 1).copied(), Some(b'/' | b'*'))
    };
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() || c == ',' {
            i += 1;
            continue;
        }
        if c == '*' {
            break;
        }
        if c == '/' && slash_terminates_at(bytes, i) {
            break;
        }
        let start = i;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c.is_whitespace() || c == ',' || c == '*' {
                break;
            }
            if c == '/' && slash_terminates_at(bytes, i) {
                break;
            }
            i += 1;
        }
        let token = &s[start..i];
        let token = token.trim_end_matches([',', ';']);
        if token.is_empty() {
            continue;
        }
        if token == "reason:" {
            break;
        }
        ids.push(token.to_string());
    }
    (ids, i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disables_named_gate() {
        assert!(is_disabled("code // hector-disable: no-todo\n", "no-todo"));
        assert!(!is_disabled("code // hector-disable: other\n", "no-todo"));
    }

    #[test]
    fn preserves_namespaced_gate_ids() {
        assert!(is_disabled(
            "x // hector-disable: python/no-print\n",
            "python/no-print"
        ));
        assert!(!is_disabled(
            "x // hector-disable: python/no-print\n",
            "python"
        ));
    }

    #[test]
    fn stops_at_block_comment_close_and_reason() {
        assert!(is_disabled("x /* hector-disable: g1 */\n", "g1"));
        assert!(is_disabled(
            "x // hector-disable: g1 reason: legacy\n",
            "g1"
        ));
        assert!(!is_disabled(
            "x // hector-disable: g1 reason: legacy\n",
            "reason:"
        ));
    }

    #[test]
    fn comma_and_whitespace_separated() {
        assert!(is_disabled("// hector-disable: a, b c\n", "a"));
        assert!(is_disabled("// hector-disable: a, b c\n", "b"));
        assert!(is_disabled("// hector-disable: a, b c\n", "c"));
    }
}
