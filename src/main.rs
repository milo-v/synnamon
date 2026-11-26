mod alert;
mod battery;
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
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Run => daemon::run(cli.config),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
