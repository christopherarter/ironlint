#![warn(clippy::cognitive_complexity)]

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
            rules,
            explain,
            print_prompt,
        } => commands::check::run(
            file,
            diff,
            session,
            format,
            &config,
            rules,
            explain,
            print_prompt,
        )?,
        Command::Trust { config } => commands::trust::run(&config)?,
        Command::Validate { config } => commands::validate::run(&config)?,
        Command::Init { dir } => commands::init::run(&dir)?,
        Command::Migrate { dir, clean } => commands::migrate::run(&dir, clean)?,
        Command::Baseline {
            action,
            config,
            scan,
        } => match action.unwrap_or(cli::BaselineAction::Record) {
            cli::BaselineAction::Record => commands::baseline::record(&config, scan)?,
            cli::BaselineAction::Refresh => commands::baseline::refresh(&config)?,
        },
        Command::Session { action } => match action {
            cli::SessionAction::Record {
                dir,
                file,
                diff,
                session_id,
            } => commands::session::record(&dir, &file, &diff, session_id)?,
            cli::SessionAction::Start { dir } => commands::session::start(&dir)?,
        },
        Command::Doctor { dir, format } => commands::doctor::run(&dir, format)?,
        Command::Explain {
            file,
            format,
            config,
        } => commands::explain::run(&file, format, &config)?,
        Command::Guide {
            file,
            format,
            config,
        } => commands::guide::run(&file, format, &config)?,
        Command::ShowResolvedConfig { config, format } => {
            commands::show_resolved_config::run(&config, format)?
        }
    };
    std::process::exit(code);
}
