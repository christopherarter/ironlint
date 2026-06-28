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
            content,
            format,
            config,
            checks,
            event,
            explain,
            allow_external_paths,
        } => commands::check::run(
            file,
            diff,
            content,
            format,
            &config,
            checks,
            event,
            explain,
            allow_external_paths,
        )?,
        Command::Trust { config } => commands::trust::run(&config)?,
        Command::Validate { config } => commands::validate::run(&config)?,
        Command::Init {
            dir,
            harnesses,
            global,
            yes,
            no_hook,
            hook_only,
            uninstall,
            dry_run,
        } => commands::init::run(
            &dir,
            &commands::init::Options {
                harnesses,
                global,
                yes,
                no_hook,
                hook_only,
                uninstall,
                dry_run,
            },
        )?,
        Command::Doctor { dir, format } => commands::doctor::run(&dir, format)?,
        Command::Explain {
            file,
            format,
            config,
        } => commands::explain::run(&file, format, &config)?,
        Command::ShowResolvedConfig { config, format } => {
            commands::show_resolved_config::run(&config, format)?
        }
        Command::Schema => commands::schema::run()?,
    };
    std::process::exit(code);
}
