use crate::config::{Config, Webhook, WebhookKind};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    Battery,
    Power,
    Report,
    SshLogin,
    LocalLogin,
    Test,
}

impl AlertKind {
    fn name(self) -> &'static str {
        match self {
            AlertKind::Battery => "battery",
            AlertKind::Power => "power",
            AlertKind::Report => "report",
            AlertKind::SshLogin => "ssh_login",
            AlertKind::LocalLogin => "local_login",
            AlertKind::Test => "test",
        }
    }
    fn color(self) -> u32 {
        match self {
            AlertKind::Battery => 0xE67E22,
            AlertKind::Power => 0xF1C40F,
            AlertKind::Report => 0x9B59B6,
            AlertKind::SshLogin => 0x3498DB,
            AlertKind::LocalLogin => 0x2ECC71,
            AlertKind::Test => 0x95A5A6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub kind: AlertKind,
    pub title: String,
    pub body: String,
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

pub fn discord_payload(a: &Alert) -> Value {
    json!({
        "content": null,
        "allowed_mentions": {"parse": []},
        "embeds": [{
            "title": truncate(&a.title, 256),
            "description": truncate(&a.body, 4000),
            "color": a.kind.color(),
        }]
    })
}

pub fn generic_payload(a: &Alert) -> Value {
    json!({"kind": a.kind.name(), "title": a.title, "body": a.body})
}

pub fn build_payload(w: &Webhook, a: &Alert) -> Value {
    match w.kind {
        WebhookKind::Discord => discord_payload(a),
        WebhookKind::Generic => generic_payload(a),
    }
}

pub fn send(w: &Webhook, a: &Alert) -> Result<()> {
    send_with_backoff(w, a, &[Duration::from_secs(1), Duration::from_secs(3)])
}

/// One attempt, plus one retry per entry in `backoffs`.
pub fn send_with_backoff(w: &Webhook, a: &Alert, backoffs: &[Duration]) -> Result<()> {
    if w.url.is_empty() {
        bail!("webhook url is not set");
    }
    let body = build_payload(w, a).to_string();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut last = anyhow!("no attempt made");
    for attempt in 0..=backoffs.len() {
        let wait = match agent
            .post(&w.url)
            .header("Content-Type", "application/json")
            .send(body.as_str())
        {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if (200..300).contains(&status) {
                    return Ok(());
                }
                last = anyhow!("webhook returned HTTP {status}");
                if status == 429 {
                    let header = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).unwrap_or("");
                    Some(parse_retry_after(header))
                } else if status >= 500 {
                    None
                } else {
                    return Err(last); // 4xx other than 429: retrying won't help
                }
            }
            Err(e) => {
                last = anyhow!("request failed: {}", e.to_string().replace(&w.url, "<webhook>"));
                None
            }
        };
        if let Some(b) = backoffs.get(attempt) {
            std::thread::sleep(wait.unwrap_or(*b));
        }
    }
    Err(last)
}

/// Seconds to wait from a `Retry-After` value: 1 s if unusable (NaN, inf, negative, text), capped at 10 s.
fn parse_retry_after(v: &str) -> Duration {
    match v.trim().parse::<f64>() {
        Ok(s) if s.is_finite() && s >= 0.0 => Duration::from_secs_f64(s.min(10.0)),
        _ => Duration::from_secs(1),
    }
}

pub fn spawn_sender(rx: Receiver<Alert>, cfg: Arc<RwLock<Config>>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        for alert in rx {
            let webhook = cfg.read().unwrap().webhook.clone();
            if webhook.url.is_empty() {
                eprintln!("alert dropped (webhook url not set): {}", alert.title);
                continue;
            }
            if let Err(e) = send(&webhook, &alert) {
                eprintln!("failed to send alert '{}': {e:#}", alert.title);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Webhook;

    fn alert(title: &str, body: &str) -> Alert {
        Alert { kind: AlertKind::SshLogin, title: title.into(), body: body.into() }
    }

    #[test]
    fn discord_payload_disables_mentions() {
        let p = discord_payload(&alert("t", "b"));
        assert_eq!(p["allowed_mentions"]["parse"], serde_json::json!([]));
    }

    #[test]
    fn discord_payload_truncates_long_title_and_body() {
        let p = discord_payload(&alert(&"t".repeat(300), &"é".repeat(5000)));
        assert!(p["embeds"][0]["title"].as_str().unwrap().chars().count() <= 256);
        assert!(p["embeds"][0]["description"].as_str().unwrap().chars().count() <= 4000);
    }

    #[test]
    fn discord_payload_keeps_everyone_text_inert() {
        let p = discord_payload(&alert("t", "hi @everyone"));
        assert_eq!(p["allowed_mentions"]["parse"], serde_json::json!([]));
        assert_eq!(p["embeds"][0]["description"], "hi @everyone");
    }

    #[test]
    fn generic_payload_has_snake_case_kind() {
        let p = generic_payload(&alert("t", "b"));
        assert_eq!(p["kind"], "ssh_login");
        assert_eq!(p["title"], "t");
        assert_eq!(p["body"], "b");
    }

    #[test]
    fn retry_after_nan_inf_and_garbage_are_safe() {
        assert_eq!(parse_retry_after("NaN"), Duration::from_secs(1));
        assert_eq!(parse_retry_after("inf"), Duration::from_secs(1));
        assert_eq!(parse_retry_after("-5"), Duration::from_secs(1));
        assert_eq!(parse_retry_after("soon"), Duration::from_secs(1));
        assert_eq!(parse_retry_after("3"), Duration::from_secs(3));
        assert_eq!(parse_retry_after("999"), Duration::from_secs(10));
    }

    #[test]
    fn send_error_does_not_contain_url() {
        let w = Webhook { url: "http://127.0.0.1:1/secret-token".into(), ..Default::default() };
        let err = send_with_backoff(&w, &alert("t", "b"), &[Duration::ZERO, Duration::ZERO]).unwrap_err();
        assert!(!format!("{err:#}").contains("secret-token"), "{err:#}");
    }
}
