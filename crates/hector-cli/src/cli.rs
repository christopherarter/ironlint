use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "hector", version, about = "Policy-enforcement pipeline for AI coding agents")]
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
        #[arg(long, default_value = "human")]
        format: OutputFormat,
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
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
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}
