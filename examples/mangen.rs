//! Generates the man page from the real CLI definition.
//!
//!     cargo run --example mangen -- target/man/synnamon.1

use clap::CommandFactory;
use clap_mangen::Man;
use std::io::Write;

#[path = "../src/cli_def.rs"]
#[allow(dead_code)]
mod cli_def;

/// Escape text for roff: hyphens must be `\-` to stay copy-pasteable, and backticks are dropped.
fn roff(s: &str) -> String {
    s.replace('\\', "\\\\").replace('-', "\\-").replace('`', "")
}

/// One entry per subcommand (recursively), with its arguments and one-line description.
fn write_commands(buf: &mut Vec<u8>, cmd: &clap::Command, prefix: &str) -> std::io::Result<()> {
    for sub in cmd.get_subcommands().filter(|s| s.get_name() != "help") {
        let mut usage = format!("{prefix} {}", sub.get_name());
        for arg in sub.get_arguments().filter(|a| a.get_id() != "help" && a.get_id() != "config") {
            match arg.get_long() {
                Some(long) => usage.push_str(&format!(" [--{long}]")),
                None => usage.push_str(&format!(" <{}>", arg.get_id().as_str().to_uppercase())),
            }
        }
        let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
        writeln!(buf, ".TP\n\\fB{}\\fR\n{}", roff(&usage), roff(&about))?;
        write_commands(buf, sub, &format!("{prefix} {}", sub.get_name()))?;
    }
    Ok(())
}

fn main() -> std::io::Result<()> {
    let out_path = std::env::args().nth(1).unwrap_or_else(|| "target/man/synnamon.1".into());
    let man = Man::new(cli_def::Cli::command());
    let mut buf: Vec<u8> = Vec::new();

    man.render_title(&mut buf)?;
    man.render_name_section(&mut buf)?;
    man.render_synopsis_section(&mut buf)?;
    man.render_description_section(&mut buf)?;
    man.render_options_section(&mut buf)?;
    writeln!(buf, ".SH COMMANDS")?;
    write_commands(&mut buf, &cli_def::Cli::command(), "synnamon")?;

    writeln!(buf, ".SH CONFIGURATION")?;
    writeln!(buf, "Settings are stored in {} and managed with \\fBsynnamon config\\fR.", roff(cli_def::DEFAULT_CONFIG_PATH))?;
    writeln!(buf, "Changes are picked up by the running daemon within a few seconds; nothing needs restarting.")?;
    writeln!(buf, "Config commands normally need \\fBsudo\\fR, because the file is readable only by root and the")?;
    writeln!(buf, "\\fBsynnamon\\fR service user (it contains the webhook URL).")?;
    for (key, default, meaning) in cli_def::CONFIG_KEYS {
        writeln!(buf, ".TP\n\\fB{}\\fR (default: {})\n{}", roff(key), roff(default), roff(meaning))?;
    }

    writeln!(buf, ".SH ALERTS")?;
    writeln!(buf, ".TP\n\\fBbattery\\fR\nBattery fell below the threshold. Sent once, and again only after it recovers.")?;
    writeln!(buf, ".TP\n\\fBpower\\fR\nExternal power was plugged in or unplugged.")?;
    writeln!(buf, ".TP\n\\fBssh_login\\fR\nA successful SSH login (user, source address, authentication method).")?;
    writeln!(buf, ".TP\n\\fBlocal_login\\fR\nA successful local login (console, tty, desktop).")?;
    writeln!(buf, ".PP\nWith a generic webhook, the JSON body has the fields kind, title and body, where kind is one of the names above (or test).")?;

    writeln!(buf, ".SH FILES")?;
    writeln!(buf, ".TP\n\\fB{}\\fR\nConfiguration (TOML).", roff(cli_def::DEFAULT_CONFIG_PATH))?;
    writeln!(buf, ".TP\n\\fB/lib/systemd/system/synnamon.service\\fR\nThe systemd unit that runs \\fBsynnamon run\\fR.")?;

    writeln!(buf, ".SH EXAMPLES")?;
    writeln!(
        buf,
        ".nf\nsudo synnamon config\nsudo synnamon config show\nsudo synnamon config set battery.threshold 30\nsudo synnamon config set power.enabled false\nsudo synnamon test\nsynnamon update \\-\\-check\n.fi"
    )?;

    man.render_version_section(&mut buf)?;
    writeln!(buf, ".SH SEE ALSO")?;
    writeln!(buf, "journalctl(1), systemctl(1)")?;

    if let Some(dir) = std::path::Path::new(&out_path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out_path, buf)?;
    eprintln!("wrote {out_path}");
    Ok(())
}
