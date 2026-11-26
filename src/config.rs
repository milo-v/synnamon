use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WebhookKind {
    #[default]
    Discord,
    Generic,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Webhook {
    pub url: String,
    pub kind: WebhookKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Battery {
    pub enabled: bool,
    pub threshold: u8,
    pub poll_secs: u64,
}

impl Default for Battery {
    fn default() -> Self {
        Battery { enabled: true, threshold: 20, poll_secs: 60 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Power {
    pub enabled: bool,
}

impl Default for Power {
    fn default() -> Self {
        Power { enabled: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    pub enabled: bool,
    pub interval_mins: u64,
}

impl Default for Report {
    fn default() -> Self {
        Report { enabled: false, interval_mins: 60 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Logins {
    pub ssh: bool,
    pub local: bool,
}

impl Default for Logins {
    fn default() -> Self {
        Logins { ssh: true, local: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub webhook: Webhook,
    pub battery: Battery,
    pub power: Power,
    pub report: Report,
    pub logins: Logins,
}

fn parse_bool(v: &str) -> Result<bool> {
    match v {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => bail!("expected true or false, got '{v}'"),
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let cfg: Config = toml::from_str(&text).context("parsing config")?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{chown, MetadataExt, PermissionsExt};
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o640))?;
            // Group-own the file like its directory (root:synnamon) so the service user can read it
            // even when this runs under sudo.
            if let Some(dir) = path.parent() {
                if let Ok(m) = std::fs::metadata(dir) {
                    let _ = chown(&tmp, None, Some(m.gid()));
                }
            }
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if !(1..=100).contains(&self.battery.threshold) {
            bail!("battery.threshold must be 1-100");
        }
        if self.battery.poll_secs < 5 {
            bail!("battery.poll_secs must be at least 5");
        }
        if !(1..=1440).contains(&self.report.interval_mins) {
            bail!("report.interval_mins must be 1-1440");
        }
        let u = &self.webhook.url;
        if !u.is_empty() && !u.starts_with("http://") && !u.starts_with("https://") {
            bail!("webhook.url must start with http:// or https://");
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        Some(match key {
            "webhook.url" => self.webhook.url.clone(),
            "webhook.kind" => match self.webhook.kind {
                WebhookKind::Discord => "discord".into(),
                WebhookKind::Generic => "generic".into(),
            },
            "battery.enabled" => self.battery.enabled.to_string(),
            "battery.threshold" => self.battery.threshold.to_string(),
            "battery.poll_secs" => self.battery.poll_secs.to_string(),
            "power.enabled" => self.power.enabled.to_string(),
            "report.enabled" => self.report.enabled.to_string(),
            "report.interval_mins" => self.report.interval_mins.to_string(),
            "logins.ssh" => self.logins.ssh.to_string(),
            "logins.local" => self.logins.local.to_string(),
            _ => return None,
        })
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let mut next = self.clone();
        match key {
            "webhook.url" => next.webhook.url = value.to_string(),
            "webhook.kind" => {
                next.webhook.kind = match value {
                    "discord" => WebhookKind::Discord,
                    "generic" => WebhookKind::Generic,
                    _ => bail!("webhook.kind must be discord or generic"),
                }
            }
            "battery.enabled" => next.battery.enabled = parse_bool(value)?,
            "battery.threshold" => next.battery.threshold = value.parse().context("not a number 1-100")?,
            "battery.poll_secs" => next.battery.poll_secs = value.parse().context("not a number")?,
            "power.enabled" => next.power.enabled = parse_bool(value)?,
            "report.enabled" => next.report.enabled = parse_bool(value)?,
            "report.interval_mins" => next.report.interval_mins = value.parse().context("not a number 1-1440")?,
            "logins.ssh" => next.logins.ssh = parse_bool(value)?,
            "logins.local" => next.logins.local = parse_bool(value)?,
            _ => bail!("unknown key '{key}'"),
        }
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub fn redacted(&self) -> Config {
        let mut c = self.clone();
        if !c.webhook.url.is_empty() {
            c.webhook.url = "***".into();
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_is_etc_synnamon() {
        assert_eq!(crate::cli_def::DEFAULT_CONFIG_PATH, "/etc/synnamon/config.toml");
    }

    #[test]
    fn config_from_before_power_existed_enables_power_alerts() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p, "[battery]\nthreshold = 30\n[logins]\nssh = false\n").unwrap();
        assert!(Config::load(&p).unwrap().power.enabled);
    }

    #[test]
    fn status_report_is_off_by_default_and_hourly() {
        let c = Config::default();
        assert!(!c.report.enabled);
        assert_eq!(c.report.interval_mins, 60);
    }

    #[test]
    fn report_interval_must_be_1_to_1440() {
        let mut c = Config::default();
        assert!(c.set("report.interval_mins", "0").is_err());
        assert!(c.set("report.interval_mins", "1441").is_err());
        assert_eq!(c.report.interval_mins, 60);
        assert!(c.set("report.interval_mins", "1440").is_ok());
    }

    #[test]
    fn load_missing_file_returns_defaults() {
        let d = tempfile::tempdir().unwrap();
        let c = Config::load(&d.path().join("nope.toml")).unwrap();
        assert_eq!(c.battery.threshold, 20);
        assert_eq!(c.battery.poll_secs, 60);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/c.toml");
        let mut c = Config::default();
        c.webhook.url = "https://example.com/hook".into();
        c.battery.threshold = 33;
        c.save(&p).unwrap();
        let l = Config::load(&p).unwrap();
        assert_eq!(l.webhook.url, "https://example.com/hook");
        assert_eq!(l.battery.threshold, 33);
    }

    #[test]
    fn set_threshold_101_errors_and_leaves_config_unchanged() {
        let mut c = Config::default();
        assert!(c.set("battery.threshold", "101").is_err());
        assert_eq!(c.battery.threshold, 20);
    }

    #[test]
    fn set_unknown_key_errors() {
        assert!(Config::default().set("nope.key", "1").is_err());
    }

    #[test]
    fn get_set_dotted_keys_roundtrip() {
        let mut c = Config::default();
        for (k, v) in [
            ("webhook.url", "http://h/x"),
            ("webhook.kind", "generic"),
            ("battery.enabled", "false"),
            ("battery.threshold", "15"),
            ("battery.poll_secs", "30"),
            ("power.enabled", "false"),
            ("report.enabled", "true"),
            ("report.interval_mins", "120"),
            ("logins.ssh", "false"),
            ("logins.local", "false"),
        ] {
            c.set(k, v).unwrap();
            assert_eq!(c.get(k).as_deref(), Some(v), "key {k}");
        }
        assert_eq!(c.get("bogus"), None);
    }

    #[test]
    fn load_invalid_toml_errors() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p, "this is [not toml").unwrap();
        assert!(Config::load(&p).is_err());
    }

    #[test]
    fn load_rejects_invalid_values() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p, "[battery]\nthreshold = 150\n").unwrap();
        assert!(Config::load(&p).is_err());
    }

    #[test]
    fn redacted_hides_url() {
        let mut c = Config::default();
        c.webhook.url = "https://secret.example/token123".into();
        let s = toml::to_string(&c.redacted()).unwrap();
        assert!(s.contains("***"));
        assert!(!s.contains("token123"));
    }
}
