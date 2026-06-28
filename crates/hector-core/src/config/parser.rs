use super::types::Config;
use anyhow::{anyhow, Context, Result};

/// Parse a `.hector.yml` (checks format).
///
/// Legacy v1/v2 configs (`schema_version:`, `rules:`, `engine:`) are rejected
/// with a curated message rather than serde's generic failure — hector 0.3
/// dropped the engine model. There is no migration path (no install base).
/// In 0.4, `gates:` was renamed to `checks:` and is also rejected as legacy.
pub fn parse_str(input: &str) -> Result<Config> {
    if let Some(key) = legacy_marker(input) {
        let lead = if key == "gates" {
            "`gates:` was renamed to `checks:` in 0.4".to_string()
        } else {
            format!("this looks like a pre-0.3 config (found `{key}:`)")
        };
        return Err(anyhow!(
            "{lead}. The 0.4 format uses a top-level `checks:` map of \
             `{{ files, run | steps }}` entries — rewrite it. \
             See specs/2026-06-28-hector-checks-pipeline-design.md"
        ));
    }
    let cfg: Config = serde_yaml::from_str(input).context("parsing hector config")?;
    for (id, check) in &cfg.checks {
        if !run_has_executable_content(&check.run) {
            return Err(anyhow!(
                "check `{id}` has a `run` with no executable command (only blank or `#` comment \
                 lines). For a multi-line script use a YAML block scalar (`run: |`); a plain or \
                 folded (`>`) scalar collapses newlines and can turn the whole script into a \
                 single comment that silently passes everything."
            ));
        }
    }
    Ok(cfg)
}

/// True if `run` has at least one line that isn't blank or a `#` comment.
///
/// A `run` made entirely of blank/comment lines (or empty) is a check that can
/// never act: `sh -c` runs it and exits 0, a silent pass. The common cause is a
/// folded/plain YAML scalar collapsing a multi-line script — see `parse_str`.
///
/// This catches the *comment-collapse* and *empty* shapes only. The other
/// folded-scalar failure mode — two real statements concatenated onto one line
/// (`grep -q X` + `exit 2` → `grep -q X exit 2`) — stays syntactically valid
/// shell, so it can't be rejected statically without a shell parser; the block
/// scalar (`run: |`) is the documented fix for both.
fn run_has_executable_content(run: &str) -> bool {
    run.lines().any(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && !trimmed.starts_with('#')
    })
}

/// Return the first top-level legacy marker key present, if any.
fn legacy_marker(input: &str) -> Option<&'static str> {
    let value: serde_yaml::Value = serde_yaml::from_str(input).ok()?;
    let map = value.as_mapping()?;
    ["gates", "schema_version", "rules", "trust"]
        .into_iter()
        .find(|k| map.contains_key(serde_yaml::Value::String((*k).into())))
}

pub fn parse_file(path: &std::path::Path) -> Result<Config> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_str(&content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_checks_config() {
        let cfg = parse_str("checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n").unwrap();
        assert!(cfg.checks.contains_key("g"));
    }

    #[test]
    fn rejects_legacy_schema_version() {
        let err = parse_str("schema_version: 2\nrules: {}\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("checks"),
            "error should point at the checks format: {err}"
        );
    }

    #[test]
    fn rejects_legacy_rules_block() {
        let err = parse_str("rules:\n  r:\n    engine: script\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("checks"),
            "error should point at the checks format: {err}"
        );
    }

    #[test]
    fn rejects_legacy_gates_key() {
        let err = parse_str("gates:\n  g:\n    files: \"*\"\n    run: \"true\"\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("checks"),
            "error should point at `checks:`: {err}"
        );
    }

    #[test]
    fn missing_checks_key_is_an_error() {
        assert!(parse_str("extends: []\n").is_err());
    }

    #[test]
    fn rejects_unknown_check_field() {
        // `exclude:` was a real field in the pre-0.3 engine model; in 0.4 a check
        // is exactly `{ files, run }`. A stale or typo'd field must be a hard
        // error, never silently dropped — dropping it makes the check behave
        // differently than the author wrote it.
        // `{:#}` is the user-visible form (the CLI prints `ERROR: {:#}`); it
        // includes the full anyhow chain down to serde's "unknown field" note.
        let err = format!(
            "{:#}",
            parse_str(
                "checks:\n  g:\n    files: \"*.ts\"\n    exclude: \"*.test.ts\"\n    run: \"true\"\n",
            )
            .unwrap_err()
        );
        assert!(
            err.contains("exclude"),
            "error must name the unknown field: {err}"
        );
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        // A typo'd top-level key (here `excludes:`) is not one of the legacy
        // markers, so the curated legacy path doesn't catch it — serde must.
        let err = format!(
            "{:#}",
            parse_str("excludes: []\nchecks:\n  g:\n    files: \"*\"\n    run: \"true\"\n")
                .unwrap_err()
        );
        assert!(
            err.contains("excludes"),
            "error must name the unknown top-level key: {err}"
        );
    }

    #[test]
    fn rejects_run_that_is_only_a_comment() {
        // The folded-scalar footgun: `run: >` with a leading `#` collapses the
        // whole multi-line script into a single comment line, yielding a check
        // that silently passes everything. Reject it at parse time.
        let err = parse_str(
            "checks:\n  g:\n    files: \"*\"\n    run: \"# block if forbidden grep -q X && exit 2 exit 0\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("`g`"),
            "error must name the offending check: {err}"
        );
    }

    #[test]
    fn rejects_empty_run() {
        // An empty (or whitespace-only) `run` is a check that can never act —
        // `sh -c ""` exits 0, a silent pass. Treat it as a config error.
        let err = parse_str("checks:\n  g:\n    files: \"*\"\n    run: \"   \"\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.to_lowercase().contains("run"),
            "error must mention the run command: {err}"
        );
    }

    #[test]
    fn accepts_multiline_block_scalar_run() {
        // Regression guard for the "multi-line run silently breaks" report: a
        // YAML literal block (`|`) is the correct idiom for a multi-statement
        // check. Newlines are preserved verbatim into the `run` string (and
        // later handed to `sh -c`), so a leading `#` comment plus real command
        // lines parses fine and keeps its line structure.
        let cfg = parse_str(
            "checks:\n  g:\n    files: \"*.py\"\n    run: |\n      # check\n      grep -q FORBIDDEN && exit 2\n      exit 0\n",
        )
        .unwrap();
        let run = &cfg.checks["g"].run;
        assert!(
            run.contains('\n'),
            "block scalar must preserve newlines: {run:?}"
        );
        assert!(
            run.contains("grep -q FORBIDDEN && exit 2"),
            "real command lines must survive intact: {run:?}"
        );
    }
}
