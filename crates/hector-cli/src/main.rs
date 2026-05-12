mod cli;
mod commands;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Check { file, diff, format, config } => {
            commands::check::run(file, diff, format, &config)?
        }
        Command::Trust { config } => commands::trust::run(&config)?,
        Command::Validate { config } => commands::validate::run(&config)?,
        Command::Init { dir } => commands::init::run(&dir)?,
    };
    std::process::exit(code);
}
