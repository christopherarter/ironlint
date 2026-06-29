//! `hector schema` — print the canonical gate-authoring guide.
//!
//! Embeds `adapters/shared/hector-config/SKILL.md` and prints its body (YAML
//! frontmatter stripped) to stdout. Read-only; never loads or trusts a config.

use anyhow::Result;

const GUIDE: &str = include_str!("../../../../adapters/shared/hector-config/SKILL.md");

/// Strip a leading `--- ... ---` YAML frontmatter block, returning the body.
/// Returns the input unchanged when there is no frontmatter.
fn strip_frontmatter(s: &str) -> &str {
    let Some(rest) = s.strip_prefix("---\n") else {
        return s;
    };
    match rest.find("\n---\n") {
        Some(idx) => rest[idx + "\n---\n".len()..].trim_start_matches('\n'),
        None => s,
    }
}

pub fn run() -> Result<i32> {
    print!("{}", strip_frontmatter(GUIDE));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_frontmatter_block() {
        let doc = "---\nname: x\ndescription: y\n---\n\n# Body\ntext\n";
        assert_eq!(strip_frontmatter(doc), "# Body\ntext\n");
    }

    #[test]
    fn passes_through_when_no_frontmatter() {
        let doc = "# Body\nno frontmatter\n";
        assert_eq!(strip_frontmatter(doc), doc);
    }

    #[test]
    fn passes_through_on_unterminated_frontmatter() {
        let doc = "---\nname: x\nno closing fence\n";
        assert_eq!(strip_frontmatter(doc), doc);
    }

    #[test]
    fn embedded_guide_has_no_frontmatter_after_strip() {
        // The real guide starts with frontmatter; the printed body must not.
        assert!(!strip_frontmatter(GUIDE).starts_with("---"));
        assert!(strip_frontmatter(GUIDE).contains("$HECTOR_FILE"));
        assert!(strip_frontmatter(GUIDE).contains("$HECTOR_TMPFILE"));
    }
}
