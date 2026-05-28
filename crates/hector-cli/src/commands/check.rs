use crate::cli::OutputFormat;
use anyhow::{Context, Result};
use hector_core::runner::{CheckInput, CheckOptions, ExplainOutcome, HectorEngine, RuleExplain};
use hector_core::verdict::{Status, Verdict};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

// The signature mirrors the clap subcommand variant one-to-one.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub fn run(
    file: Option<PathBuf>,
    diff: Option<PathBuf>,
    content: Option<String>,
    session: bool,
    format: OutputFormat,
    config: &Path,
    rules: Vec<String>,
    explain: bool,
    print_prompt: bool,
    emit_semantic_payload: bool,
    allow_external_paths: bool,
) -> Result<i32> {
    // Load once: build with the non-rule options, then validate `--rule`
    // against the loaded config and store the validated set in place, rather
    // than paying trust-verify + extends DFS + YAML parse for a separate
    // probe load.
    let options = CheckOptions {
        rules: HashSet::new(),
        explain,
        emit_semantic_payload,
        allow_external_paths,
    };
    let mut engine = match HectorEngine::builder().with_options(options).load(config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            return Ok(1);
        }
    };
    if let Some(code) = validate_rule_filter(&engine, &rules) {
        return Ok(code);
    }
    engine.set_rule_filter(rules.into_iter().collect());

    if print_prompt {
        return run_print_prompt(&engine, file, diff, content);
    }
    if session {
        return run_session(&engine, config, format);
    }

    match (file, diff) {
        (Some(f), None) => run_file(&engine, f, content, format, explain),
        (None, Some(_)) if emit_semantic_payload => {
            eprintln!("ERROR: --emit-semantic-payload is not supported with --diff yet (multi-file envelope aggregation is a follow-up)");
            Ok(1)
        }
        (None, Some(d)) => run_diff(&engine, &d, format, explain),
        _ => {
            eprintln!("ERROR: provide exactly one of --file or --diff");
            Ok(1)
        }
    }
}

/// Stop-hook path: evaluate the recorded session. A deferred envelope is
/// emitted as JSON and exits 0 (the subagent decides the verdict), leaving
/// `session.json` intact for a possible re-run; otherwise the verdict is
/// emitted and the session cleared on Pass/Warn.
fn run_session(engine: &HectorEngine, config: &Path, format: OutputFormat) -> Result<i32> {
    let dir = config
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let state_path = dir.join(".hector/session.json");
    let state = hector_core::session_state::SessionState::load(&state_path)?;
    let report = engine.check_session_with_options(&state)?;
    if let Some(d) = &report.deferred {
        emit_deferred(d, format)?;
        return Ok(0);
    }
    emit(&report.verdict, format)?;
    if should_clear_session(report.verdict.status) {
        hector_core::session_state::SessionState::clear(&state_path)?;
    }
    Ok(exit_code(&report.verdict))
}

/// Check a single file. `--content` overrides the disk read so PreToolUse
/// adapters can gate on proposed pre-write content; clap guarantees
/// `--content` implies `--file`.
fn run_file(
    engine: &HectorEngine,
    file: PathBuf,
    content: Option<String>,
    format: OutputFormat,
    explain: bool,
) -> Result<i32> {
    let content = match content {
        Some(c) => resolve_content_value(c)?,
        None => std::fs::read_to_string(&file)
            .with_context(|| format!("failed to read {}", file.display()))?,
    };
    let report = engine.check_with_explain(CheckInput::File {
        path: file,
        content,
    })?;
    if explain {
        print_explain(&report.explain);
    }
    if let Some(d) = &report.deferred {
        // The runner never builds an envelope alongside a terminal verdict;
        // re-check here so a future runner change can't silently leak one.
        if matches!(report.verdict.status, Status::Block | Status::InternalError) {
            emit(&report.verdict, format)?;
            return Ok(exit_code(&report.verdict));
        }
        emit_deferred(d, format)?;
        return Ok(0);
    }
    emit(&report.verdict, format)?;
    Ok(exit_code(&report.verdict))
}

/// Check every changed file in a unified diff and aggregate the verdicts.
/// An empty diff is an error; a pure-deletion diff is a clean Pass (deleted
/// files can't be read and no rule fires on absent content).
fn run_diff(
    engine: &HectorEngine,
    diff: &Path,
    format: OutputFormat,
    explain: bool,
) -> Result<i32> {
    let unified_diff = std::fs::read_to_string(diff)?;
    let changed = hector_core::diff::parser::parse_unified(&unified_diff)?;
    if changed.is_empty() {
        eprintln!("ERROR: no changed files in diff");
        return Ok(1);
    }
    let has_non_deleted = changed
        .iter()
        .any(|f| f.op != hector_core::diff::ChangeOp::Deleted);
    if !has_non_deleted {
        let verdict = Verdict::from_violations(vec![], vec![], 0);
        emit(&verdict, format)?;
        return Ok(0);
    }

    let mut aggregated_violations = Vec::new();
    let mut aggregated_passed = Vec::new();
    let mut aggregated_explain: Vec<RuleExplain> = Vec::new();
    let mut elapsed_ms: u64 = 0;
    for f in changed {
        if f.op == hector_core::diff::ChangeOp::Deleted {
            continue;
        }
        let per_file_diff = build_single_file_diff(&unified_diff, &f.path);
        let r = engine.check_with_explain(CheckInput::Diff {
            file: f.path,
            unified_diff: per_file_diff,
        })?;
        elapsed_ms = elapsed_ms.saturating_add(r.verdict.elapsed_ms);
        aggregated_violations.extend(r.verdict.violations);
        aggregated_passed.extend(r.verdict.passed_checks);
        aggregated_explain.extend(r.explain);
    }
    let verdict = Verdict::from_violations(aggregated_violations, aggregated_passed, elapsed_ms);
    if explain {
        print_explain(&aggregated_explain);
    }
    emit(&verdict, format)?;
    Ok(exit_code(&verdict))
}

/// Refuse `--rule <unknown>` at the CLI boundary so callers see a clear
/// error before any rule runs.
fn validate_rule_filter(engine: &HectorEngine, rules: &[String]) -> Option<i32> {
    if rules.is_empty() {
        return None;
    }
    let known: HashSet<&str> = engine.config_rule_ids().collect();
    let unknown: Vec<&String> = rules
        .iter()
        .filter(|id| !known.contains(id.as_str()))
        .collect();
    if unknown.is_empty() {
        None
    } else {
        let names: Vec<&str> = unknown.iter().map(|s| s.as_str()).collect();
        eprintln!("ERROR: unknown rule id(s): {}", names.join(", "));
        Some(1)
    }
}

/// Render the `--explain` rows to stderr so stdout (JSON) stays clean.
fn print_explain(rows: &[RuleExplain]) {
    for row in rows {
        let outcome = match &row.outcome {
            ExplainOutcome::Fire => "fire".to_string(),
            ExplainOutcome::Pass => "pass".to_string(),
            ExplainOutcome::Dispatched => "dispatched".to_string(),
            ExplainOutcome::Skipped { reason } => format!("skipped {reason}"),
        };
        let engine_name = match row.engine {
            hector_core::config::EngineKind::Script => "script",
            hector_core::config::EngineKind::Ast => "ast",
            hector_core::config::EngineKind::Semantic => "semantic",
            hector_core::config::EngineKind::Session => "session",
        };
        eprintln!("{} {} {}", row.rule_id, engine_name, outcome);
    }
}

/// Render the (system, user) prompt for every in-scope semantic rule and
/// exit 0 — no engine dispatch, so no HTTP request reaches the LLM.
fn run_print_prompt(
    engine: &HectorEngine,
    file: Option<PathBuf>,
    diff: Option<PathBuf>,
    content: Option<String>,
) -> Result<i32> {
    let input = match (file, diff) {
        (Some(f), None) => {
            // `--content` overrides the disk read so operators can preview
            // prompts against proposed content before it lands on disk.
            let content = match content {
                Some(c) => resolve_content_value(c)?,
                None => std::fs::read_to_string(&f)
                    .with_context(|| format!("failed to read {}", f.display()))?,
            };
            CheckInput::File { path: f, content }
        }
        (None, Some(d)) => {
            let unified_diff = std::fs::read_to_string(&d)?;
            let changed = hector_core::diff::parser::parse_unified(&unified_diff)?;
            let Some(first) = changed.into_iter().next() else {
                eprintln!("ERROR: no changed files in diff");
                return Ok(1);
            };
            let per_file = build_single_file_diff(&unified_diff, &first.path);
            CheckInput::Diff {
                file: first.path,
                unified_diff: per_file,
            }
        }
        _ => {
            eprintln!("ERROR: provide exactly one of --file or --diff");
            return Ok(1);
        }
    };
    let prompts = engine.render_semantic_prompts(input)?;
    if prompts.is_empty() {
        eprintln!("no semantic rule in scope; nothing to render");
    }
    for p in &prompts {
        println!("# rule: {}", p.rule_id);
        println!("## system");
        println!("{}", p.system);
        println!("## user");
        println!("{}", p.user);
    }
    Ok(0)
}

fn exit_code(v: &Verdict) -> i32 {
    match v.status {
        Status::Block => 2,
        Status::InternalError => 3,
        // Pass, Warn, and any future #[non_exhaustive] variants all exit 0
        // (fail-open): unknown status values must never accidentally block.
        _ => 0,
    }
}

/// Clear the session file only on Pass/Warn, so a Block or InternalError
/// verdict leaves `.hector/session.json` intact for re-inspection.
fn should_clear_session(status: Status) -> bool {
    matches!(status, Status::Pass | Status::Warn)
}

fn emit_deferred(
    d: &hector_core::verdict_deferred::DeferredVerdict,
    format: OutputFormat,
) -> Result<()> {
    match format {
        OutputFormat::Json | OutputFormat::Human => {
            // For deferred envelopes, JSON is always the wire format —
            // the adapter parses it via `jq`. `--format human` falls back
            // to JSON because the envelope is machine-only.
            println!("{}", serde_json::to_string_pretty(d)?);
        }
    }
    Ok(())
}

fn emit(v: &Verdict, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(v)?);
        }
        OutputFormat::Human => {
            for vio in &v.violations {
                eprintln!(
                    "{}: [{}] {}{}",
                    vio.severity_human(),
                    vio.rule_id,
                    vio.file,
                    vio.line.map(|l| format!(":{l}")).unwrap_or_default()
                );
                eprintln!("  {}", vio.message);
            }
            println!(
                "{}",
                match v.status {
                    Status::Pass => "pass",
                    Status::Warn => "warn",
                    Status::Block => "block",
                    Status::InternalError => "internal_error",
                    // #[non_exhaustive]: future variants surface as "unknown".
                    _ => "unknown",
                }
            );
        }
    }
    Ok(())
}

/// Extract the path from a `+++ b/<path>[\t<timestamp>]` header line.
///
/// POSIX `diff -u` appends `\t<timestamp>` after the path; split at the
/// first tab and discard the timestamp before comparing paths.
fn header_path(line: &str) -> Option<&str> {
    line.strip_prefix("+++ b/").map(|p| {
        p.split('\t')
            .next()
            .unwrap_or(p)
            .trim_end_matches(['\n', '\r'])
    })
}

/// Extract the path from a `--- a/<path>[\t<timestamp>]` header line.
///
/// Symmetric to `header_path`: strips the `--- a/` prefix, splits at the
/// first tab (POSIX timestamp), and trims trailing `\r`/`\n`.
fn minus_header_path(line: &str) -> Option<&str> {
    line.strip_prefix("--- a/").map(|p| {
        p.split('\t')
            .next()
            .unwrap_or(p)
            .trim_end_matches(['\n', '\r'])
    })
}

/// Slice a multi-file unified diff down to the hunks for a single file.
///
/// A file's section starts at the `--- a/<path>` header that precedes its
/// `+++ b/<path>` line and ends at the next `--- a/...` header (or EOF). We
/// scan for the matching `+++ b/<path>` and, when found, walk backwards to
/// include the preceding `--- a/...` line so the slice is a syntactically
/// well-formed diff in its own right.
///
/// The preceding `--- a/` line is included only when its path matches the
/// target; a foreign header from the previous file is omitted (and
/// `parse_unified` tolerates an absent `---` header).
fn build_single_file_diff(full: &str, file: &Path) -> String {
    let target = file.display().to_string();
    // `split_inclusive` preserves line terminators so we can round-trip the
    // slice without re-emitting newlines.
    let lines: Vec<&str> = full.split_inclusive('\n').collect();

    // Locate the `+++ b/<path>` for the target file, stripping any
    // POSIX-style tab+timestamp before the path comparison.
    let plus_idx = lines
        .iter()
        .position(|line| header_path(line).is_some_and(|p| p == target));
    let Some(plus_idx) = plus_idx else {
        return String::new();
    };

    // Include the preceding `--- a/...` header only when its parsed path
    // matches the target; a foreign header from the previous file would
    // otherwise corrupt this slice.
    let header_idx =
        if plus_idx > 0 && minus_header_path(lines[plus_idx - 1]).is_some_and(|p| p == target) {
            plus_idx - 1
        } else {
            plus_idx
        };

    // Walk forward until the next `--- ` header (start of another file) or
    // end of input.
    let end_idx = lines
        .iter()
        .enumerate()
        .skip(plus_idx + 1)
        .find_map(|(i, line)| line.starts_with("--- ").then_some(i))
        .unwrap_or(lines.len());

    lines[header_idx..end_idx].concat()
}

/// Resolve a `--content` value: a literal `-` reads bytes from stdin
/// (the documented adapter path for large content); any other string is
/// the content itself.
///
/// `Read::read_to_string` already rejects non-UTF-8 bytes — surface that
/// as an `anyhow` error with context rather than panicking. Stdin is
/// allowed to be empty (an empty pre-write file is legitimate, e.g.
/// `write_file` creating a new empty source).
fn resolve_content_value(value: String) -> Result<String> {
    if value == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("failed to read --content from stdin (expected UTF-8)")?;
        Ok(buf)
    } else {
        Ok(value)
    }
}

trait SeverityHuman {
    fn severity_human(&self) -> &'static str;
}

impl SeverityHuman for hector_core::verdict::Violation {
    fn severity_human(&self) -> &'static str {
        match self.severity {
            hector_core::verdict::Severity::Error => "error",
            hector_core::verdict::Severity::Warning => "warn",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn slice_preserves_each_files_hunks() {
        let full = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-x\n+fn a() {}\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1 @@\n-x\n+fn b() { panic!(); }\n";
        let slice_a = build_single_file_diff(full, &PathBuf::from("src/a.rs"));
        let slice_b = build_single_file_diff(full, &PathBuf::from("src/b.rs"));
        assert_eq!(
            slice_a,
            "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-x\n+fn a() {}\n"
        );
        assert_eq!(
            slice_b,
            "--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1 @@\n-x\n+fn b() { panic!(); }\n"
        );
        // Sanity: each slice parses cleanly into a single ChangedFile.
        let parsed_a = hector_core::diff::parser::parse_unified(&slice_a).unwrap();
        let parsed_b = hector_core::diff::parser::parse_unified(&slice_b).unwrap();
        assert_eq!(parsed_a.len(), 1);
        assert_eq!(parsed_a[0].path, PathBuf::from("src/a.rs"));
        assert_eq!(parsed_b.len(), 1);
        assert_eq!(parsed_b[0].path, PathBuf::from("src/b.rs"));
    }

    #[test]
    fn slice_returns_empty_for_unknown_file() {
        let full = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n+x\n";
        let slice = build_single_file_diff(full, &PathBuf::from("src/missing.rs"));
        assert_eq!(slice, "");
    }

    /// `build_single_file_diff` must not include a foreign `--- a/<other>`
    /// header in the slice when it doesn't match the target file.
    #[test]
    fn slice_drops_mismatched_minus_header() {
        let diff = "--- a/src/a.rs\n+++ b/src/b.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n";
        let slice = build_single_file_diff(diff, &PathBuf::from("src/b.rs"));
        // The foreign `--- a/src/a.rs` header must not appear in the slice.
        assert!(
            !slice.starts_with("--- a/src/a.rs"),
            "slice must not include the mismatched --- header; got: {slice:?}"
        );
        // The slice must still be parseable and yield exactly src/b.rs.
        let files = hector_core::diff::parser::parse_unified(&slice).expect("re-parse");
        assert_eq!(
            files.len(),
            1,
            "mismatched --- header must not introduce a phantom file"
        );
        assert_eq!(files[0].path, PathBuf::from("src/b.rs"));
    }

    #[test]
    fn should_clear_session_on_pass_and_warn_only() {
        assert!(should_clear_session(Status::Pass));
        assert!(should_clear_session(Status::Warn));
        assert!(!should_clear_session(Status::Block));
        // InternalError leaves the edit unresolved — keep the session.
        assert!(!should_clear_session(Status::InternalError));
    }
}
