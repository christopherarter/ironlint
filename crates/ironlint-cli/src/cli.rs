use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "ironlint",
    version,
    about = "Policy-enforcement pipeline for AI coding agents"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the pipeline against a file, a diff, or — with neither — sweep the repo.
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
        #[arg(long, default_value = ".ironlint.yml")]
        config: PathBuf,
        /// Evaluate only this check id. Repeatable; multiple flags OR'd.
        #[arg(long = "check", action = clap::ArgAction::Append)]
        checks: Vec<String>,
        /// What triggered this check, surfaced to checks as $IRONLINT_EVENT.
        /// Defaults to `write` for `--file`/`--diff`. Not valid with a bare
        /// repo-wide sweep, which derives each check's lifecycle from its
        /// `on:` list. Restricted to the two ABI values; an unknown value is
        /// rejected at the arg layer so typos never reach `$IRONLINT_EVENT`.
        #[arg(
            long,
            value_parser = clap::builder::PossibleValuesParser::new(["write", "pre-commit"])
        )]
        event: Option<String>,
        /// After the verdict, print a per-gate outcome report to stderr.
        /// Rows cover per-file (write-lifecycle) checks; batched pre-commit
        /// checks emit no rows.
        #[arg(long)]
        explain: bool,
        /// Allow checking files whose canonical path falls outside the
        /// directory containing the config file. Disabled by default to
        /// prevent wrappers from inadvertently running policy against
        /// arbitrary host files.
        #[arg(long, default_value_t = false)]
        allow_external_paths: bool,
        /// Run the named `--check` id(s) against `--file` even if the path is
        /// outside their `files` glob. Scope-only; requires `--check`.
        #[arg(long, default_value_t = false)]
        force: bool,
        /// Exit nonzero when no checks matched the file. Without this, a
        /// glob typo that matches nothing prints a visible `pass (no checks
        /// matched ...)` note but still exits 0 — fine for local use. In CI,
        /// pass `--require-match` so a silent policy bypass fails the build.
        #[arg(long, default_value_t = false)]
        require_match: bool,
    },
    /// Bless this config + its `.ironlint/scripts/` scripts in the out-of-repo trust store.
    Trust {
        #[arg(long, default_value = ".ironlint.yml")]
        config: PathBuf,
    },
    /// Parse and validate the config without running any gates.
    Validate {
        #[arg(long, default_value = ".ironlint.yml")]
        config: PathBuf,
        #[arg(long, default_value = "human")]
        format: OutputFormat,
    },
    /// Detect stack and scaffold a starter .ironlint.yml, then wire ironlint's hook
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
        /// Remove ironlint hooks and materialized artifacts.
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
        /// Directory containing `.ironlint.yml`. Defaults to cwd.
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
        #[arg(long, default_value = ".ironlint.yml")]
        config: PathBuf,
    },
    /// Print the post-extends merged check set.
    ///
    /// Read-only. Does not run any check. Default format prints each check
    /// with its files glob(s) and run command, annotated by origin.
    ShowResolvedConfig {
        #[arg(long, default_value = ".ironlint.yml")]
        config: PathBuf,
        #[arg(long, default_value = "tsv")]
        format: ShowFormat,
    },
    /// Print the canonical check-authoring guide (the `.ironlint.yml` schema and
    /// patterns). Read-only.
    Schema,
    /// Update ironlint to the latest release.
    ///
    /// Re-runs the dist installer (`ironlint-cli-installer.sh` on Unix,
    /// `.ps1` on Windows) in place. The installer is idempotent, so this also
    /// covers the already-current case (it re-runs but exits 0). Exit codes:
    /// `0` on a successful update; `1` on failure — including when this build
    /// wasn't installed by the ironlint installer (Homebrew/cargo/source
    /// builds) and so can't self-update, in which case it prints the
    /// channel-specific command that will.
    Update,
    /// Live TUI over the telemetry log: a stream of check runs and a per-check
    /// explorer. Read-only; requires an interactive terminal.
    Watch {
        /// Directory containing `.ironlint.yml` / `.ironlint/log.jsonl`. Defaults to cwd.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Decide whether a Bash command may run. Reads the command on stdin.
    /// Exit 0 = allow (empty stdout); exit 2 = block (reason on stdout).
    /// Not a check, not trust-gated; works with no .ironlint.yml present.
    GateBash,
    /// Architecture-enforcement layer rules: check imports, render the dependency
    /// graph, or explain why a file is blocked.
    Arch {
        #[command(subcommand)]
        sub: ArchSub,
    },
}

#[derive(Debug, Subcommand)]
pub enum ArchSub {
    /// Check the whole graph, or a single proposed write, against layer rules.
    Check {
        #[arg(long)]
        layers: Option<PathBuf>,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(
            long,
            value_parser = clap::builder::PossibleValuesParser::new(["write", "pre-commit"])
        )]
        event: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
        /// Path to a manifest of sibling proposed files (tab-separated
        /// `file_path\tcontent_path` lines). Lets the arch check see
        /// cross-file imports within a single atomic patch (Bug 1).
        #[arg(long)]
        proposed_manifest: Option<PathBuf>,
    },
    /// Render the dependency graph (DOT by default).
    Graph {
        #[arg(long)]
        layers: Option<PathBuf>,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long, conflicts_with = "json")]
        dot: bool,
        #[arg(long, conflicts_with = "dot")]
        json: bool,
    },
    /// Explain every violation whose importer is `<path>`.
    Why {
        path: PathBuf,
        #[arg(long)]
        layers: Option<PathBuf>,
        #[arg(long)]
        root: Option<PathBuf>,
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
