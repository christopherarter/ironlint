use std::collections::BTreeMap;

/// Maps line number → set of rule_ids disabled on that line.
#[derive(Debug, Default)]
pub struct DisableMap {
    by_line: BTreeMap<u32, Vec<String>>,
}

impl DisableMap {
    pub fn from_source(src: &str) -> Self {
        let mut map = Self::default();
        for (i, line) in src.lines().enumerate() {
            let line_no = (i as u32) + 1;
            for rule_id in parse_disable_directives(line) {
                map.by_line.entry(line_no).or_default().push(rule_id);
            }
        }
        map
    }

    pub fn is_disabled(&self, line: u32, rule_id: &str) -> bool {
        self.by_line
            .get(&line)
            .map(|rules| rules.iter().any(|r| r == rule_id))
            .unwrap_or(false)
    }
}

fn parse_disable_directives(line: &str) -> Vec<String> {
    let marker = "hector-disable:";
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(idx) = rest.find(marker) {
        let after = &rest[idx + marker.len()..];
        let (ids, consumed) = collect_rule_ids(after);
        out.extend(ids);
        rest = &after[consumed..];
    }
    out
}

/// Collect comma- or whitespace-separated rule IDs after `hector-disable:`.
/// Stops at end-of-input, the literal token `reason:`, or any `*`/`/` terminator
/// (block-comment closers). Returns the rule IDs and the number of bytes consumed
/// from `s` so the outer loop can continue scanning for additional directives.
fn collect_rule_ids(s: &str) -> (Vec<String>, usize) {
    let mut ids = Vec::new();
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() || c == ',' {
            i += 1;
            continue;
        }
        if c == '*' || c == '/' {
            break;
        }
        // Start of a token. Find its end.
        let start = i;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c.is_whitespace() || c == ',' || c == '*' || c == '/' {
                break;
            }
            i += 1;
        }
        let token = &s[start..i];
        // Strip trailing punctuation defensively (e.g. semicolons), though the
        // walker above already excludes `,` from token bodies.
        let token = token.trim_end_matches([',', ';']);
        if token.is_empty() {
            continue;
        }
        // The `reason:` keyword terminates the rule-id list. Don't push it.
        if token == "reason:" {
            break;
        }
        ids.push(token.to_string());
    }
    (ids, i)
}
