use crate::alert::{Alert, AlertKind};
use crate::config::{Config, Logins};
use serde_json::Value;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginEvent {
    Ssh { user: String, ip: String, method: String },
    LogindSession { session_id: String, user: String },
}

fn message_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => {
            let bytes: Option<Vec<u8>> = a.iter().map(|n| n.as_u64().and_then(|n| u8::try_from(n).ok())).collect();
            bytes.map(|b| String::from_utf8_lossy(&b).into_owned())
        }
        _ => None,
    }
}

fn field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// Parse one `journalctl -o json` line into a login event, if it is one.
pub fn parse_line(line: &str) -> Option<LoginEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    // SYSLOG_IDENTIFIER is spoofable by any local user (`logger -t sshd ...`); _UID is added by
    // journald itself, so require that the sender is root.
    if field(&v, "_UID") != Some("0") {
        return None;
    }
    let ident = field(&v, "SYSLOG_IDENTIFIER").or_else(|| field(&v, "_COMM"))?;
    let msg = message_text(v.get("MESSAGE")?)?;
    let t: Vec<&str> = msg.split_whitespace().collect();
    match ident {
        // "Accepted <method> for <user> from <ip> port <p> ssh2 ..."
        // OpenSSH 9.8+ logs from the `sshd-session` process.
        "sshd" | "sshd-session" if t.len() >= 6 && t[0] == "Accepted" && t[2] == "for" && t[4] == "from" => {
            Some(LoginEvent::Ssh { user: t[3].into(), ip: t[5].into(), method: t[1].into() })
        }
        // "New session <id> of user <user>."
        "systemd-logind" if t.len() >= 6 && t[0] == "New" && t[1] == "session" && t[3] == "of" && t[4] == "user" => {
            Some(LoginEvent::LogindSession { session_id: t[2].into(), user: t[5].trim_end_matches('.').into() })
        }
        _ => None,
    }
}

pub trait SessionInfo {
    fn is_local(&self, session_id: &str) -> bool;
}

pub fn remote_is_no(output: &str) -> bool {
    output.lines().any(|l| l.trim() == "Remote=no")
}

/// Asks `loginctl`; any failure counts as "not local" so we never double-alert an SSH login.
pub struct Loginctl;

impl SessionInfo for Loginctl {
    fn is_local(&self, session_id: &str) -> bool {
        std::process::Command::new("loginctl")
            .args(["show-session", session_id, "-p", "Remote"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| remote_is_no(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or(false)
    }
}

pub fn to_alert(ev: &LoginEvent, cfg: &Logins, info: &dyn SessionInfo) -> Option<Alert> {
    match ev {
        LoginEvent::Ssh { user, ip, method } if cfg.ssh => Some(Alert {
            kind: AlertKind::SshLogin,
            title: "SSH login".into(),
            body: format!("{user} logged in from {ip} via {method}"),
        }),
        LoginEvent::LogindSession { session_id, user } if cfg.local && info.is_local(session_id) => Some(Alert {
            kind: AlertKind::LocalLogin,
            title: "Local login".into(),
            body: format!("{user} logged in locally (session {session_id})"),
        }),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
pub fn spawn(cfg: Arc<RwLock<Config>>, tx: SyncSender<Alert>) -> JoinHandle<()> {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    std::thread::spawn(move || {
        let mut backoff = Duration::from_secs(1);
        loop {
            let started = Instant::now();
            match Command::new("journalctl")
                .args(["-f", "-o", "json", "--since", "now"])
                .stdout(Stdio::piped())
                .spawn()
            {
                Ok(mut child) => {
                    if let Some(out) = child.stdout.take() {
                        for line in BufReader::new(out).split(b'\n').flatten() {
                            let line = String::from_utf8_lossy(&line);
                            if let Some(ev) = parse_line(&line) {
                                let logins = cfg.read().unwrap().logins.clone();
                                if let Some(a) = to_alert(&ev, &logins, &Loginctl) {
                                    crate::daemon::send_alert(&tx, a);
                                }
                            }
                        }
                    }
                    let _ = child.wait();
                    eprintln!("logins: journalctl exited; restarting in {backoff:?}");
                }
                Err(e) => eprintln!("logins: cannot start journalctl: {e}; retrying in {backoff:?}"),
            }
            if started.elapsed() > Duration::from_secs(60) {
                backoff = Duration::from_secs(1);
            }
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    })
}

#[cfg(not(target_os = "linux"))]
pub fn spawn(_cfg: Arc<RwLock<Config>>, _tx: SyncSender<Alert>) -> JoinHandle<()> {
    std::thread::spawn(|| eprintln!("logins: journal monitoring is only available on Linux"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(name: &str) -> String {
        std::fs::read_to_string(format!("tests/fixtures/{name}")).unwrap()
    }

    struct Fake(bool);
    impl SessionInfo for Fake {
        fn is_local(&self, _: &str) -> bool {
            self.0
        }
    }

    fn ssh() -> LoginEvent {
        LoginEvent::Ssh { user: "milo".into(), ip: "1.2.3.4".into(), method: "publickey".into() }
    }
    fn sess() -> LoginEvent {
        LoginEvent::LogindSession { session_id: "7".into(), user: "milo".into() }
    }
    fn on() -> Logins {
        Logins { ssh: true, local: true }
    }

    #[test]
    fn parses_ssh_publickey_accept() {
        assert_eq!(
            parse_line(&fx("ssh_publickey.json")),
            Some(LoginEvent::Ssh { user: "milo".into(), ip: "192.168.1.50".into(), method: "publickey".into() })
        );
    }

    #[test]
    fn parses_ssh_password_accept() {
        assert_eq!(
            parse_line(&fx("ssh_password.json")),
            Some(LoginEvent::Ssh { user: "milo".into(), ip: "10.0.0.7".into(), method: "password".into() })
        );
    }

    #[test]
    fn parses_sshd_session_identifier() {
        assert_eq!(
            parse_line(&fx("ssh_session_ident.json")),
            Some(LoginEvent::Ssh { user: "milo".into(), ip: "192.168.1.50".into(), method: "publickey".into() })
        );
    }

    #[test]
    fn ignores_lines_from_non_root_senders() {
        assert_eq!(parse_line(&fx("spoofed_user_logger.json")), None);
    }

    #[test]
    fn ignores_failed_password() {
        assert_eq!(parse_line(&fx("ssh_failed.json")), None);
    }

    #[test]
    fn ignores_invalid_user() {
        assert_eq!(parse_line(&fx("ssh_invalid_user.json")), None);
    }

    #[test]
    fn parses_logind_new_session() {
        assert_eq!(
            parse_line(&fx("logind_new_session.json")),
            Some(LoginEvent::LogindSession { session_id: "7".into(), user: "milo".into() })
        );
    }

    #[test]
    fn byte_array_message_is_decoded() {
        assert_eq!(
            parse_line(&fx("ssh_bytes.json")),
            Some(LoginEvent::Ssh { user: "milo".into(), ip: "1.2.3.4".into(), method: "publickey".into() })
        );
    }

    #[test]
    fn invalid_utf8_bytes_do_not_panic() {
        let line = r#"{"SYSLOG_IDENTIFIER":"sshd","MESSAGE":[255,254,65]}"#;
        assert_eq!(parse_line(line), None);
    }

    #[test]
    fn malformed_json_returns_none() {
        assert_eq!(parse_line("{not json"), None);
        assert_eq!(parse_line(""), None);
    }

    #[test]
    fn unrelated_unit_returns_none() {
        let line = r#"{"SYSLOG_IDENTIFIER":"cron","MESSAGE":"Accepted publickey for x from 1.1.1.1 port 1"}"#;
        assert_eq!(parse_line(line), None);
    }

    #[test]
    fn ssh_event_alerts_when_enabled() {
        let a = to_alert(&ssh(), &on(), &Fake(true)).unwrap();
        assert_eq!(a.kind, AlertKind::SshLogin);
        assert_eq!(a.title, "SSH login");
        assert!(a.body.contains("milo") && a.body.contains("1.2.3.4") && a.body.contains("publickey"));
    }

    #[test]
    fn ssh_event_silent_when_disabled() {
        assert!(to_alert(&ssh(), &Logins { ssh: false, local: true }, &Fake(true)).is_none());
    }

    #[test]
    fn remote_logind_session_suppressed() {
        assert!(to_alert(&sess(), &on(), &Fake(false)).is_none());
    }

    #[test]
    fn local_logind_session_alerts() {
        let a = to_alert(&sess(), &on(), &Fake(true)).unwrap();
        assert_eq!(a.kind, AlertKind::LocalLogin);
        assert_eq!(a.title, "Local login");
        assert!(a.body.contains("milo"));
    }

    #[test]
    fn local_disabled_suppresses() {
        assert!(to_alert(&sess(), &Logins { ssh: true, local: false }, &Fake(true)).is_none());
    }

    #[test]
    fn loginctl_output_parsing() {
        assert!(remote_is_no("Remote=no\n"));
        assert!(!remote_is_no("Remote=yes\n"));
        assert!(!remote_is_no(""));
    }
}
