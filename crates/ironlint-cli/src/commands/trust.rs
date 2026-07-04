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
        format!("  gates: {}", summary.gates.len()),
    ];
    for gate in &summary.gates {
        lines.push(format!("    - {gate}"));
    }

    if !summary.scripts.is_empty() {
        lines.push(format!("  scripts: {}", summary.scripts.len()));
        for script in &summary.scripts {
            lines.push(format!("    - {script}"));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn summary(gates: Vec<&str>, scripts: Vec<&str>) -> BlessedSummary {
        BlessedSummary {
            config_path: PathBuf::from("/abs/.ironlint.yml"),
            config_hash: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd"
                .to_string(),
            gates: gates.into_iter().map(String::from).collect(),
            scripts: scripts.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn render_summary_truncates_hash_to_16_hex_chars() {
        let out = render_summary(&summary(vec![], vec![]));
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
    fn render_summary_lists_gates_and_omits_empty_scripts_block() {
        let out = render_summary(&summary(vec!["a.sh", "b.sh"], vec![]));
        assert!(out.contains("gates: 2"));
        assert!(out.contains("    - a.sh"));
        assert!(out.contains("    - b.sh"));
        assert!(
            !out.contains("scripts:"),
            "scripts block must be omitted entirely when there are none: {out}"
        );
    }

    #[test]
    fn render_summary_prints_scripts_block_when_nonempty() {
        let out = render_summary(&summary(vec![], vec!["scripts/lint.sh"]));
        assert!(out.contains("gates: 0"));
        assert!(out.contains("scripts: 1"));
        assert!(out.contains("    - scripts/lint.sh"));
    }

    #[test]
    fn render_summary_guards_against_a_short_hash() {
        let mut s = summary(vec![], vec![]);
        s.config_hash = "sha256:abcd".to_string();
        let out = render_summary(&s);
        assert!(
            out.contains("config sha256: abcd\n"),
            "a short hash must not index-panic, just print what's there: {out}"
        );
    }
}
