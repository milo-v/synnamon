mod alert;
mod battery;
mod cli;
mod config;
mod daemon;
mod logins;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "synnamon", about = "Lightweight system monitor and alerter")]
struct Cli {
    /// Path to the config file
    #[arg(long, global = true, default_value = config::DEFAULT_PATH)]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the monitoring daemon
    Run,
    /// Manage configuration (no arguments: guided setup)
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Send a test alert through the configured webhook
    Test,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Print one setting
    Get { key: String },
    /// Change one setting
    Set { key: String, value: String },
    /// Print all settings (webhook URL hidden)
    Show,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Run => daemon::run(cli.config),
        Command::Config { action } => cli::cmd_config(&cli.config, action),
        Command::Test => cli::cmd_test(&cli.config),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
