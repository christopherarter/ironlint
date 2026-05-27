use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "hector",
    version,
    about = "Policy-enforcement pipeline for AI coding agents"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the pipeline against a file or diff.
    Check {
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        diff: Option<PathBuf>,
        /// Evaluate this proposed post-edit content instead of reading
        /// `--file` from disk. Pass `-` to read the bytes from stdin
        /// (recommended for any content larger than a few KB; argv has
        /// OS-level size limits).
        ///
        /// Designed for PreToolUse adapters — Reasonix, OpenCode
        /// `tool.execute.before`, deepseek-reasonix and similar — that
        /// need to gate on the proposed edit *before* it lands on disk.
        /// Requires `--file` so scope matching, baseline matching, and
        /// AST language detection all key off the real project path.
        ///
        /// Limitation: `engine: script` rules invoke an external command
        /// against the on-disk file via `{file}`/`HECTOR_FILE`, so they
        /// see the *current* disk content, not the proposed content.
        /// AST, semantic, and `hector-disable:` directives all read from
        /// `--content` correctly. See spec
        /// `specs/2026-05-25-reasonix-adapter.md` §5.
        #[arg(
            long,
            value_name = "STRING_OR_DASH",
            requires = "file",
            conflicts_with = "diff",
            conflicts_with = "session"
        )]
        content: Option<String>,
        #[arg(long)]
        session: bool,
        #[arg(long, default_value = "human")]
        format: OutputFormat,
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
        /// Evaluate only this rule id. Repeatable; multiple flags OR'd.
        #[arg(long = "rule", action = clap::ArgAction::Append)]
        rules: Vec<String>,
        /// After the verdict, print a per-rule outcome report to stderr.
        #[arg(long)]
        explain: bool,
        /// For semantic rules in scope, render the prompt to stdout and exit 0
        /// without dispatching to the LLM. Debug-only.
        #[arg(long = "print-prompt")]
        print_prompt: bool,
        /// H1: instead of dispatching `engine: semantic` and
        /// `engine: session` rules to the configured LLM, collect them
        /// into a `DeferredVerdict` JSON envelope for an in-session
        /// Claude Code subagent to evaluate. Adapter-internal.
        #[arg(
            long = "emit-semantic-payload",
            conflicts_with = "session",
            conflicts_with = "print_prompt"
        )]
        emit_semantic_payload: bool,
        /// C4: allow checking files whose canonical path falls outside
        /// the directory containing the config file. Disabled by default
        /// to prevent wrappers from inadvertently running policy against
        /// arbitrary host files.
        #[arg(long, default_value_t = false)]
        allow_external_paths: bool,
    },
    /// Compute the trust fingerprint and write it to the config.
    Trust {
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
    },
    /// Parse and validate the config without running any rules.
    Validate {
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
    },
    /// Detect stack and scaffold a starter .hector.yml
    Init {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Rewrite .bully.yml to .hector.yml (schema v1 -> v2). Move .bully/ -> .hector/.
    Migrate {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Delete .bully.yml after migration.
        #[arg(long)]
        clean: bool,
    },
    /// Record current violations to .hector/baseline.json (silenced from future runs).
    ///
    /// Without an action, defaults to `record`. The action subcommands are
    /// `record` (capture current violations) and `refresh` (re-hash every
    /// stored entry against current file content). Existing
    /// `hector baseline` invocations keep working — the subcommand is
    /// optional.
    Baseline {
        #[command(subcommand)]
        action: Option<BaselineAction>,
        #[arg(long, default_value = ".hector.yml", global = true)]
        config: PathBuf,
        /// (record mode) Glob filter restricting which files are scanned.
        #[arg(long, global = true)]
        scan: Option<String>,
    },
    /// Session-state management (used by Claude Code adapter hooks).
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Diagnose the local install, config, trust, engine availability, and adapter wiring.
    ///
    /// Read-only. Exits 0 if every check passes or only warns; exits 1 on any failure.
    Doctor {
        /// Directory containing `.hector.yml`. Defaults to cwd.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Output format. `human` (default) prints a checklist; `json` prints a
        /// machine-readable report — see `docs/doctor.md` for the schema.
        #[arg(long, default_value = "human")]
        format: OutputFormat,
    },
    /// Show which rules are in scope for `<file>` and which skip-pattern
    /// (if any) suppresses it. Read-only — no engine runs, no LLM is
    /// called, no telemetry is written.
    Explain {
        /// Path to inspect. Relative to cwd.
        file: PathBuf,
        #[arg(long, default_value = "human")]
        format: OutputFormat,
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
    },
    /// List the rules whose scope matches `<file>` with their description
    /// and severity. Read-only — see `explain` for full scope reporting.
    Guide {
        /// Path to inspect. Relative to cwd.
        file: PathBuf,
        #[arg(long, default_value = "human")]
        format: OutputFormat,
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
    },
    /// Print the post-extends merged rule set.
    ///
    /// Read-only. Does not run any rule. Default format is TSV with the
    /// columns: `id<TAB>engine<TAB>severity<TAB>scope<TAB>fix_hint<TAB>origin`.
    /// `--format yaml` prints the canonical merged config (sans `trust:`
    /// and `extends:`); `--format json` prints the same shape as JSON
    /// with each rule annotated by its origin.
    ShowResolvedConfig {
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
        #[arg(long, default_value = "tsv")]
        format: ShowFormat,
    },
    /// Append one `semantic_verdict` record to `.hector/log.jsonl`.
    ///
    /// Adapter-internal: consumed by the Claude Code interpreter skill
    /// after a subagent evaluates a deferred semantic rule. See
    /// `docs/record-verdict.md` for the wire-format contract.
    RecordVerdict {
        /// Rule id this verdict is for (single occurrence — one verdict per call).
        #[arg(long = "rule")]
        rule: String,
        /// Verdict value: `pass` or `violation`. Other values rejected at parse time.
        #[arg(long = "verdict", value_enum)]
        verdict: crate::commands::record_verdict::VerdictValue,
        /// Optional file path the verdict pertains to. When omitted, the
        /// appended record has `file: null`.
        #[arg(long = "file")]
        file: Option<String>,
        /// Directory containing `.hector/log.jsonl`. Defaults to cwd.
        #[arg(long = "dir", default_value = ".")]
        dir: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, Subcommand)]
pub enum BaselineAction {
    /// Record current violations to .hector/baseline.json.
    Record,
    /// Re-hash every baseline entry against current file content.
    Refresh,
}

#[derive(Debug, Subcommand)]
pub enum SessionAction {
    /// Append an edit record to .hector/session.json.
    Record {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        file: PathBuf,
        #[arg(long, allow_hyphen_values = true)]
        diff: String,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// Stamp a `session_init` record into the telemetry log.
    Start {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ShowFormat {
    Tsv,
    Yaml,
    Json,
}
