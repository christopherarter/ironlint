mod cli;
mod commands;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Check {
            file,
            diff,
            session,
            format,
            config,
        } => commands::check::run(file, diff, session, format, &config)?,
        Command::Trust { config } => commands::trust::run(&config)?,
        Command::Validate { config } => commands::validate::run(&config)?,
        Command::Init { dir } => commands::init::run(&dir)?,
        Command::Migrate { dir, clean } => commands::migrate::run(&dir, clean)?,
        Command::Baseline { config, scan } => commands::baseline::run(&config, scan)?,
        Command::Session { action } => match action {
            cli::SessionAction::Record {
                dir,
                file,
                diff,
                session_id,
            } => commands::session::record(&dir, &file, &diff, session_id)?,
        },
    };
    std::process::exit(code);
}
