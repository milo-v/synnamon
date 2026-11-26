use crate::alert::{self, Alert, AlertKind};
use crate::config::Config;
use anyhow::{bail, Result};
use std::io::{BufRead, Write};
use std::path::Path;

fn read_line(input: &mut dyn BufRead) -> Result<String> {
    let mut s = String::new();
    if input.read_line(&mut s)? == 0 {
        bail!("input ended before the wizard finished");
    }
    Ok(s.trim().to_string())
}

fn normalize_bool(s: &str) -> String {
    match s.to_lowercase().as_str() {
        "y" | "yes" | "true" => "true".into(),
        "n" | "no" | "false" => "false".into(),
        other => other.to_string(),
    }
}

/// Ask until the answer is accepted by `Config::set`. Empty input keeps the current value.
fn ask(cfg: &mut Config, key: &str, label: &str, shown: &str, input: &mut dyn BufRead, out: &mut dyn Write) -> Result<()> {
    loop {
        write!(out, "{label} [{shown}]: ")?;
        out.flush()?;
        let answer = read_line(input)?;
        if answer.is_empty() {
            return Ok(());
        }
        let value = if key == "battery.enabled" || key == "power.enabled" || key.starts_with("logins.") {
            normalize_bool(&answer)
        } else {
            answer
        };
        match cfg.set(key, &value) {
            Ok(()) => return Ok(()),
            Err(e) => writeln!(out, "  invalid: {e}")?,
        }
    }
}

pub fn wizard(cfg: &mut Config, input: &mut dyn BufRead, out: &mut dyn Write) -> Result<()> {
    let yn = |b: bool| if b { "Y/n" } else { "y/N" };
    let url_state = if cfg.webhook.url.is_empty() { "not set" } else { "set" };
    writeln!(out, "Synnamon setup. Press Enter to keep the value in brackets.")?;
    ask(cfg, "webhook.url", "Webhook URL", &format!("({url_state})"), input, out)?;
    let kind = cfg.get("webhook.kind").unwrap();
    ask(cfg, "webhook.kind", "Webhook type (discord/generic)", &kind, input, out)?;
    let s = yn(cfg.battery.enabled).to_string();
    ask(cfg, "battery.enabled", "Monitor battery?", &s, input, out)?;
    let s = cfg.battery.threshold.to_string();
    ask(cfg, "battery.threshold", "Alert when battery below (%)", &s, input, out)?;
    let s = yn(cfg.power.enabled).to_string();
    ask(cfg, "power.enabled", "Alert when power is plugged/unplugged?", &s, input, out)?;
    let s = yn(cfg.logins.ssh).to_string();
    ask(cfg, "logins.ssh", "Alert on SSH logins?", &s, input, out)?;
    let s = yn(cfg.logins.local).to_string();
    ask(cfg, "logins.local", "Alert on local logins?", &s, input, out)?;
    Ok(())
}

pub fn cmd_show(cfg: &Config) -> String {
    toml::to_string_pretty(&cfg.redacted()).unwrap_or_default()
}

pub fn save_config(cfg: &Config, path: &Path) -> Result<()> {
    cfg.save(path).map_err(|e| match e.downcast_ref::<std::io::Error>() {
        Some(io) if io.kind() == std::io::ErrorKind::PermissionDenied => {
            anyhow::anyhow!("Cannot write {}: try running with sudo", path.display())
        }
        _ => e,
    })?;
    println!("Saved {}. A running daemon picks up changes automatically.", path.display());
    Ok(())
}

pub fn cmd_config(path: &Path, action: Option<crate::cli_def::ConfigAction>) -> Result<()> {
    use crate::cli_def::ConfigAction::*;
    let mut cfg = Config::load(path)?;
    match action {
        None => {
            wizard(&mut cfg, &mut std::io::stdin().lock(), &mut std::io::stdout())?;
            save_config(&cfg, path)
        }
        Some(Get { key }) => match cfg.get(&key) {
            Some(v) if key == "webhook.url" && !v.is_empty() => {
                println!("(set; use `config show` — url is hidden)");
                Ok(())
            }
            Some(v) => {
                println!("{v}");
                Ok(())
            }
            None => bail!("unknown key '{key}'"),
        },
        Some(Set { key, value }) => {
            cfg.set(&key, &value)?;
            save_config(&cfg, path)
        }
        Some(Show) => {
            print!("{}", cmd_show(&cfg));
            Ok(())
        }
    }
}

pub fn cmd_test(path: &Path) -> Result<()> {
    let cfg = Config::load(path)?;
    let a = Alert { kind: AlertKind::Test, title: "Synnamon test".into(), body: "Webhook is working.".into() };
    alert::send(&cfg.webhook, &a)?;
    println!("Test alert sent.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_wizard(cfg: &mut Config, input: &str) -> anyhow::Result<String> {
        let mut out = Vec::new();
        wizard(cfg, &mut input.as_bytes(), &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn wizard_enter_keeps_defaults() {
        let mut c = Config::default();
        run_wizard(&mut c, "\n\n\n\n\n\n\n").unwrap();
        assert_eq!(c.battery.threshold, 20);
        assert!(c.battery.enabled && c.power.enabled && c.logins.ssh && c.logins.local);
        assert_eq!(c.webhook.url, "");
    }

    #[test]
    fn wizard_sets_values() {
        let mut c = Config::default();
        run_wizard(&mut c, "https://example.com/h\ngeneric\nn\n30\nno\nn\ny\n").unwrap();
        assert!(!c.power.enabled);
        assert_eq!(c.webhook.url, "https://example.com/h");
        assert_eq!(c.get("webhook.kind").unwrap(), "generic");
        assert!(!c.battery.enabled);
        assert_eq!(c.battery.threshold, 30);
        assert!(!c.logins.ssh);
        assert!(c.logins.local);
    }

    #[test]
    fn wizard_reprompts_on_invalid_threshold() {
        let mut c = Config::default();
        run_wizard(&mut c, "\n\n\nabc\n150\n30\n\n\n\n").unwrap();
        assert_eq!(c.battery.threshold, 30);
    }

    #[test]
    fn wizard_never_prints_existing_url() {
        let mut c = Config::default();
        c.webhook.url = "https://discord.example/api/webhooks/tok123".into();
        let out = run_wizard(&mut c, "\n\n\n\n\n\n\n").unwrap();
        assert!(!out.contains("tok123"));
        assert!(out.contains("(set)"));
    }

    #[test]
    fn wizard_errors_on_eof_instead_of_looping() {
        let mut c = Config::default();
        assert!(run_wizard(&mut c, "").is_err());
    }

    #[test]
    fn show_redacts_url() {
        let mut c = Config::default();
        c.webhook.url = "https://discord.example/api/webhooks/tok123".into();
        let s = cmd_show(&c);
        assert!(s.contains("***") && !s.contains("tok123"));
    }
}
