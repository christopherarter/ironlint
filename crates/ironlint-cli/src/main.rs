#![warn(clippy::cognitive_complexity)]

mod cli;
mod commands;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};

fn main() -> Result<()> {
    // Parse with the fallible API so a *usage* error (typo'd flag, missing
    // value, bare invocation) can be remapped to exit 1 (config/usage tier)
    // instead of clap's default 2. Exit 2 is reserved for a real **Block**
    // verdict; adapters map exit 2 to a policy block and show its stdout as
    // the reason, so a typo must never look like a block. `e.use_stderr()` is
    // false for `--help`/`--version` (which should still exit 0) and true for
    // genuine parse errors — so help stays exit 0 while errors become exit 1.
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            e.print().expect("write clap output to stdout/stderr");
            std::process::exit(i32::from(e.use_stderr()));
        }
    };
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
            force,
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
            force,
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
        Command::Update => commands::update::run()?,
        Command::Watch { dir } => commands::watch::run(&dir)?,
    };
    std::process::exit(code);
}
