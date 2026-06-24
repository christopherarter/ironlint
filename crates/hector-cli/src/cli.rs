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
        /// Evaluate only this gate id. Repeatable; multiple flags OR'd.
        #[arg(long = "gate", action = clap::ArgAction::Append)]
        gates: Vec<String>,
        /// What triggered this check, surfaced to gates as $HECTOR_EVENT.
        #[arg(long, default_value = "manual")]
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
    /// Detect stack and scaffold a starter .hector.yml
    Init {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
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
    /// Show which gates apply to `<file>` and their run commands.
    ///
    /// Read-only — no gate runs, no telemetry is written.
    Explain {
        /// Path to inspect. Relative to cwd.
        file: PathBuf,
        #[arg(long, default_value = "human")]
        format: OutputFormat,
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
    },
    /// Print the post-extends merged gate set.
    ///
    /// Read-only. Does not run any gate. Default format prints each gate
    /// with its files glob(s) and run command, annotated by origin.
    ShowResolvedConfig {
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
        #[arg(long, default_value = "tsv")]
        format: ShowFormat,
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
