//! Command-line definition. Kept free of other crate modules so `examples/mangen.rs` can include
//! this file directly and generate the man page from it.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/synnamon/config.toml";

/// (key, default, meaning) for every setting. Feeds `--help` and the man page.
pub const CONFIG_KEYS: &[(&str, &str, &str)] = &[
    ("webhook.url", "(empty)", "Webhook to post to. Secret: never printed by `config show`."),
    ("webhook.kind", "discord", "`discord` or `generic`."),
    ("battery.enabled", "true", "Alert when the battery is low."),
    ("battery.threshold", "20", "Alert when the battery falls below this percent (1-100)."),
    ("battery.poll_secs", "60", "How often to check the battery, in seconds (minimum 5)."),
    ("power.enabled", "true", "Alert when external power is plugged in or unplugged."),
    ("report.enabled", "false", "Send a periodic status report (uptime, load, memory, disk, battery)."),
    ("report.interval_mins", "60", "Minutes between status reports (1-1440)."),
    ("logins.ssh", "true", "Alert on successful SSH logins."),
    ("logins.local", "true", "Alert on successful local logins."),
];

fn keys_help() -> String {
    let mut s = String::from("Settings (key, default, meaning):\n");
    for (key, default, meaning) in CONFIG_KEYS {
        s.push_str(&format!("  {key:<20} {default:<8} {meaning}\n"));
    }
    s.push_str("\nConfig commands usually need sudo: the file is readable only by root and the synnamon user.");
    s
}

#[derive(Parser)]
#[command(
    name = "synnamon",
    version,
    about = "Lightweight system monitor and alerter",
    long_about = "Lightweight system monitor and alerter. Sends a Discord or generic HTTP webhook message \
                  on low battery, power plugged/unplugged, and successful SSH or local logins."
)]
pub struct Cli {
    /// Path to the config file
    #[arg(long, global = true, default_value = DEFAULT_CONFIG_PATH)]
    pub config: PathBuf,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run the monitoring daemon (normally started by systemd)
    Run,
    /// Manage configuration (no arguments: guided setup)
    #[command(after_help = keys_help())]
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Send a test alert through the configured webhook
    Test,
    /// Check GitHub for a newer release and install it
    Update {
        /// Only report whether an update is available
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Print one setting
    Get { key: String },
    /// Change one setting
    #[command(after_help = keys_help())]
    Set { key: String, value: String },
    /// Print all settings (webhook URL hidden)
    Show,
}
