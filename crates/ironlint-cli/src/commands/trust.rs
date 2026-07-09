use anyhow::Result;
use ironlint_core::trust::BlessedSummary;
use std::path::Path;

pub fn run(config: &Path) -> Result<i32> {
    let config = match crate::commands::config::resolve_config(config) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("error: {msg}");
            return Ok(1);
        }
    };
    // Bless, then summarize what was blessed. Chaining both fallible steps
    // through one error arm keeps the one error voice (lowercase `error:`,
    // flattened chain via `{:#}`) for either failure and emits no success
    // output — no half-printed `trusted:` followed by a raw anyhow dump.
    match ironlint_core::trust::bless(&config)
        .and_then(|()| ironlint_core::trust::blessed_summary(&config))
    {
        Ok(summary) => {
            println!("trusted: {}", config.display());
            println!("{}", render_summary(&summary));
            Ok(0)
        }
        Err(e) => {
            eprintln!("error: {:#}", e);
            Ok(1)
        }
    }
}

/// Render a [`BlessedSummary`] as the indented, human-facing block printed
/// after the `trusted: <path>` line. Kept separate from `run` (and taking a
/// borrowed summary rather than doing its own I/O) so it is unit-testable
/// without a subprocess.
fn render_summary(summary: &BlessedSummary) -> String {
    let hex = summary
        .config_hash
        .strip_prefix("sha256:")
        .unwrap_or(&summary.config_hash);
    let short_len = hex.len().min(16);

    let mut lines = vec![
        format!("  config sha256: {}", &hex[..short_len]),
        format!("  checks: {}", summary.checks),
        format!("  scripts: {}", summary.scripts.len()),
    ];
    for script in &summary.scripts {
        lines.push(format!("    - {script}"));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn summary(checks: usize, scripts: Vec<&str>) -> BlessedSummary {
        BlessedSummary {
            config_path: PathBuf::from("/abs/.ironlint.yml"),
            config_hash: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd"
                .to_string(),
            checks,
            scripts: scripts.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn render_summary_truncates_hash_to_16_hex_chars() {
        let out = render_summary(&summary(0, vec![]));
        assert!(
            out.contains("config sha256: 0123456789abcdef\n"),
            "hash must be truncated to the first 16 hex chars: {out}"
        );
        assert!(
            !out.contains("0123456789abcdef0"),
            "must not print more than 16 hex chars: {out}"
        );
    }

    #[test]
    fn render_summary_prints_checks_and_scripts() {
        let out = render_summary(&summary(6, vec!["lint.sh", "no-todo.sh"]));
        assert!(out.contains("checks: 6"), "must print checks count: {out}");
        assert!(
            out.contains("scripts: 2"),
            "must print scripts count: {out}"
        );
        assert!(out.contains("    - lint.sh"));
        assert!(out.contains("    - no-todo.sh"));
    }

    #[test]
    fn render_summary_prints_scripts_block_even_when_empty() {
        // The scripts block is the policy-directory listing — always present,
        // even when empty, so the operator sees "scripts: 0" explicitly rather
        // than wondering whether the surface was omitted.
        let out = render_summary(&summary(2, vec![]));
        assert!(out.contains("checks: 2"));
        assert!(
            out.contains("scripts: 0"),
            "scripts: 0 is printed, not omitted: {out}"
        );
    }

    #[test]
    fn render_summary_guards_against_a_short_hash() {
        let mut s = summary(0, vec![]);
        s.config_hash = "sha256:abcd".to_string();
        let out = render_summary(&s);
        assert!(
            out.contains("config sha256: abcd\n"),
            "a short hash must not index-panic, just print what's there: {out}"
        );
    }
}
