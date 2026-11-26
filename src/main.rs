mod alert;
mod battery;
mod cli;
mod cli_def;
mod config;
mod daemon;
mod logins;
mod power;
mod update;

use clap::Parser;
use cli_def::{Cli, Command};

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Run => daemon::run(cli.config),
        Command::Config { action } => cli::cmd_config(&cli.config, action),
        Command::Test => cli::cmd_test(&cli.config),
        Command::Update { check } => update::run(check),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
