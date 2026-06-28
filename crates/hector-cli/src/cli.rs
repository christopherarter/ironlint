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
        #[arg(
            long,
            value_name = "STRING_OR_DASH",
            requires = "file",
            conflicts_with = "diff"
        )]
        content: Option<String>,
        #[arg(long, default_value = "human")]
        format: OutputFormat,
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
        /// Evaluate only this check id. Repeatable; multiple flags OR'd.
        #[arg(long = "check", action = clap::ArgAction::Append)]
        checks: Vec<String>,
        /// What triggered this check, surfaced to checks as $HECTOR_EVENT.
        /// Restricted to the four ABI values; an unknown value is rejected
        /// at the arg layer so typos never reach `$HECTOR_EVENT`.
        #[arg(
            long,
            default_value = "manual",
            value_parser = clap::builder::PossibleValuesParser::new(["edit", "write", "pre-commit", "manual"])
        )]
        event: String,
        /// After the verdict, print a per-gate outcome report to stderr.
        #[arg(long)]
        explain: bool,
        /// Allow checking files whose canonical path falls outside the
        /// directory containing the config file. Disabled by default to
        /// prevent wrappers from inadvertently running policy against
        /// arbitrary host files.
        #[arg(long, default_value_t = false)]
        allow_external_paths: bool,
    },
    /// Bless this config + its `.hector/gates/` scripts in the out-of-repo trust store.
    Trust {
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
    },
    /// Parse and validate the config without running any gates.
    Validate {
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
    },
    /// Detect stack and scaffold a starter .hector.yml, then wire hector's hook
    /// into your coding agents.
    Init {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Harness(es) to wire up (repeatable); `all` selects every supported
        /// harness. Omit to auto-detect and confirm.
        #[arg(long = "harness", value_name = "NAME")]
        harnesses: Vec<String>,
        /// Patch user-level settings instead of project-local.
        #[arg(long)]
        global: bool,
        /// Skip the install confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Scaffold the config but install no hooks (legacy behavior).
        #[arg(long)]
        no_hook: bool,
        /// Skip config scaffolding; only wire hooks.
        #[arg(long)]
        hook_only: bool,
        /// Remove hector hooks and materialized artifacts.
        #[arg(long)]
        uninstall: bool,
        /// Print intended changes without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Diagnose the local install, config, and adapter wiring.
    ///
    /// Read-only. Exits 0 if every check passes or only warns; exits 1 on any failure.
    Doctor {
        /// Directory containing `.hector.yml`. Defaults to cwd.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Output format. `human` (default) prints a checklist; `json` prints a
        /// machine-readable report — see `docs/operating/diagnostics.md` for the schema.
        #[arg(long, default_value = "human")]
        format: OutputFormat,
    },
    /// Show which checks apply to `<file>` and their run commands.
    ///
    /// Read-only — no check runs, no telemetry is written.
    Explain {
        /// Path to inspect. Relative to cwd.
        file: PathBuf,
        #[arg(long, default_value = "human")]
        format: OutputFormat,
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
    },
    /// Print the post-extends merged check set.
    ///
    /// Read-only. Does not run any check. Default format prints each check
    /// with its files glob(s) and run command, annotated by origin.
    ShowResolvedConfig {
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
        #[arg(long, default_value = "tsv")]
        format: ShowFormat,
    },
    /// Print the canonical gate-authoring guide (the `.hector.yml` schema and
    /// patterns). Read-only.
    Schema,
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
