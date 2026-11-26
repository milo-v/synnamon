use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

const DEFAULT_REPO: &str = "milo-v/synnamon";

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// "v1.2.3", "1.2.3" or "1.2.3-rc1" -> (1, 2, 3). Pre-release suffixes are ignored.
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut it = core.split('.');
    let v = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    it.next().is_none().then_some(v)
}

/// True only when `latest` parses and is strictly greater than `current`.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

pub fn latest_tag_from_json(body: &str) -> Result<String> {
    let v: Value = serde_json::from_str(body).context("unexpected response from GitHub")?;
    match v.get("tag_name").and_then(Value::as_str) {
        Some(t) if !t.is_empty() => Ok(t.to_string()),
        _ => bail!("no release found (GitHub said: {})", v.get("message").and_then(Value::as_str).unwrap_or("nothing")),
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).build().into()
}

fn is_root() -> bool {
    Command::new("id").arg("-u").output().map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0").unwrap_or(false)
}

/// `synnamon update [--check]`: compare with the latest GitHub release and, if newer, run that
/// release's install.sh (which verifies the package checksum and installs it with apt).
pub fn run(check_only: bool) -> Result<()> {
    let repo = std::env::var("SYNNAMON_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string());
    let current = current_version();

    let api = format!("https://api.github.com/repos/{repo}/releases/latest");
    let body = agent()
        .get(&api)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("could not reach GitHub to check for updates")?
        .body_mut()
        .read_to_string()?;
    let tag = latest_tag_from_json(&body)?;

    if !is_newer(current, &tag) {
        println!("synnamon {current} is up to date (latest release: {tag}).");
        return Ok(());
    }
    println!("Update available: {current} -> {tag}");
    if check_only {
        println!("Run `sudo synnamon update` to install it.");
        return Ok(());
    }
    if !is_root() {
        bail!("installing needs root: run `sudo synnamon update`");
    }

    let script = agent()
        .get(&format!("https://github.com/{repo}/releases/download/{tag}/install.sh"))
        .call()
        .context("could not download the installer")?
        .body_mut()
        .read_to_string()?;

    let mut child = Command::new("bash")
        .arg("-s")
        .env("SYNNAMON_VERSION", &tag)
        .env("SYNNAMON_REPO", &repo)
        .stdin(Stdio::piped())
        .spawn()
        .context("could not start bash")?;
    child.stdin.take().context("no stdin")?.write_all(script.as_bytes())?;
    if !child.wait()?.success() {
        bail!("installer failed; nothing was changed");
    }
    println!("Updated to {tag}. The service was restarted if it was running.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_minor_is_newer() {
        assert!(is_newer("0.1.0", "v0.2.0"));
    }

    #[test]
    fn same_version_is_not_newer() {
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.2.0", "v0.2.0"));
    }

    #[test]
    fn older_is_not_newer() {
        assert!(!is_newer("0.2.0", "0.1.9"));
    }

    #[test]
    fn versions_compare_numerically_not_as_text() {
        assert!(!is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("0.9.0", "0.10.0"));
    }

    #[test]
    fn unparsable_latest_is_not_newer() {
        assert!(!is_newer("0.1.0", "garbage"));
        assert!(!is_newer("0.1.0", ""));
    }

    #[test]
    fn prerelease_suffix_is_ignored() {
        assert!(is_newer("0.1.0", "0.2.0-rc1"));
    }

    #[test]
    fn latest_tag_is_read_from_api_json() {
        let body = r#"{"id":1,"tag_name":"v0.2.0","name":"v0.2.0","assets":[]}"#;
        assert_eq!(latest_tag_from_json(body).unwrap(), "v0.2.0");
    }

    #[test]
    fn latest_tag_errors_on_missing_field_or_bad_json() {
        assert!(latest_tag_from_json(r#"{"message":"Not Found"}"#).is_err());
        assert!(latest_tag_from_json("<html>").is_err());
    }
}
