use super::types::Config;
use anyhow::{anyhow, Context, Result};

/// Parse a `.ironlint.yml` (checks format).
///
/// Legacy v1/v2 configs (`schema_version:`, `rules:`, `engine:`) are rejected
/// with a curated message rather than serde's generic failure — ironlint 0.3
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
             See specs/2026-06-28-ironlint-checks-pipeline-design.md"
        ));
    }
    if let Some(id) = duplicate_check_id(input) {
        return Err(anyhow!(
            "duplicate check id `{id}` — each check id must be unique. Found `{id}:` more \
             than once under `checks:`; rename or remove one (a copy-paste or merge-conflict \
             resolution can silently disarm a check this way)."
        ));
    }
    let cfg: Config = serde_yaml::from_str(input).context("parsing ironlint config")?;
    for (id, check) in &cfg.checks {
        match (&check.run, &check.steps) {
            (Some(_), Some(_)) => {
                return Err(anyhow!(
                    "check `{id}` has both `run` and `steps` — use one \
                     (a single `run` is one-step sugar)"
                ))
            }
            (None, None) => {
                return Err(anyhow!(
                    "check `{id}` has neither `run` nor `steps` — a check must do something"
                ))
            }
            (Some(run), None) => guard_run(id, None, run)?,
            (None, Some(steps)) => {
                for (i, step) in steps.iter().enumerate() {
                    guard_run(id, Some(i), &step.run)?;
                }
            }
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

/// Validate that a `run` string contains at least one executable line.
///
/// `step_idx` names the step position when validating inside `steps` (None for
/// the top-level `run` field). Wraps `run_has_executable_content` and emits a
/// location-aware error so the author knows exactly which check (and which step)
/// is broken.
fn guard_run(id: &str, step_idx: Option<usize>, run: &str) -> anyhow::Result<()> {
    if !run_has_executable_content(run) {
        let location = match step_idx {
            None => format!("check `{id}`"),
            Some(i) => format!("check `{id}` step {i}"),
        };
        return Err(anyhow!(
            "{location} has a `run` with no executable command (only blank or `#` comment \
             lines). For a multi-line script use a YAML block scalar (`run: |`); a plain or \
             folded (`>`) scalar collapses newlines and can turn the whole script into a \
             single comment that silently passes everything."
        ));
    }
    Ok(())
}

/// True if `line` is a check-id key line: indented exactly 2 spaces.
///
/// Check ids sit at 2-space indent directly under `checks:`; a check's body
/// fields (`files:`, `run:`, `steps:`, ...) and any block-scalar content sit
/// at 4+ spaces, so this predicate never mistakes a body line for an id.
fn is_check_id_line(line: &str) -> bool {
    line.starts_with("  ") && !line.starts_with("   ")
}

/// Return the id of the first check key that appears more than once under
/// `checks:`, if any.
///
/// `serde_yaml::Value` (and the `Config` map it deserializes into) silently
/// collapses duplicate mapping keys, last-write-wins — so a duplicate must be
/// caught by scanning the raw text before that collapse happens. This is a
/// line-based pre-scan of the `checks:` block: it collects each 2-space
/// indented key (a check id) until the block ends (a non-indented,
/// non-blank, non-comment line, or end of input) and reports the first
/// repeat.
fn duplicate_check_id(input: &str) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let mut in_checks_block = false;
    for line in input.lines() {
        if line.starts_with("checks:") {
            in_checks_block = true;
            continue;
        }
        if !in_checks_block || line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if !line.starts_with(' ') {
            break;
        }
        if !is_check_id_line(line) {
            continue;
        }
        let id = line[2..].split(':').next().unwrap_or("").trim();
        if !id.is_empty() && !seen.insert(id.to_string()) {
            return Some(id.to_string());
        }
    }
    None
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
    fn duplicate_check_ids_are_rejected() {
        let err = parse_str(
            "checks:\n  dup:\n    files: \"*.rs\"\n    run: \"exit 2\"\n  dup:\n    files: \"*.rs\"\n    run: \"true\"\n"
        ).unwrap_err().to_string();
        assert!(
            err.contains("dup"),
            "error should name the duplicated id: {err}"
        );
    }

    #[test]
    fn unique_check_ids_with_multiline_run_are_not_false_positives() {
        // Regression guard: a check's own body fields (`files:`, `run:`) and a
        // multi-line block-scalar `run` (which can itself contain lines that
        // *look* like `key:` pairs, e.g. the `# comment` line below) must never
        // be mistaken for a repeated check id. Check ids sit at exactly 2-space
        // indent; body fields and block-scalar content sit at 4+ spaces.
        let cfg = parse_str(
            "checks:\n  a:\n    files: \"*.py\"\n    run: |\n      # comment\n      grep -q FORBIDDEN && exit 2\n      exit 0\n  b:\n    files: \"*.rs\"\n    run: \"true\"\n",
        )
        .unwrap();
        assert!(cfg.checks.contains_key("a"));
        assert!(cfg.checks.contains_key("b"));
    }

    #[test]
    fn unique_check_ids_with_steps_are_not_false_positives() {
        // `steps:` entries repeat the `run:` key at 6-space indent (list items
        // under a 4-space `steps:`); that repetition must not be mistaken for a
        // duplicated check id either.
        let cfg = parse_str(
            "checks:\n  a:\n    files: \"*\"\n    steps:\n      - run: \"true\"\n      - run: \"true\"\n  b:\n    files: \"*\"\n    run: \"true\"\n",
        )
        .unwrap();
        assert!(cfg.checks.contains_key("a"));
        assert!(cfg.checks.contains_key("b"));
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
        let run = cfg.checks["g"].run.as_deref().unwrap();
        assert!(
            run.contains('\n'),
            "block scalar must preserve newlines: {run:?}"
        );
        assert!(
            run.contains("grep -q FORBIDDEN && exit 2"),
            "real command lines must survive intact: {run:?}"
        );
    }

    // --- Phase 2: run-xor-steps + per-step validation ---

    #[test]
    fn rejects_check_with_both_run_and_steps() {
        let err = parse_str(
            "checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n    steps:\n      - run: \"true\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("`g`") && err.contains("run") && err.contains("steps"),
            "{err}"
        );
    }

    #[test]
    fn rejects_check_with_neither_run_nor_steps() {
        let err = parse_str("checks:\n  g:\n    files: \"*\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("`g`"), "{err}");
    }

    #[test]
    fn rejects_step_with_comment_only_run() {
        let err =
            parse_str("checks:\n  g:\n    files: \"*\"\n    steps:\n      - run: \"# nope\"\n")
                .unwrap_err()
                .to_string();
        assert!(err.contains("`g`"), "{err}");
    }
}
