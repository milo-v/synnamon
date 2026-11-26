use crate::alert::{Alert, AlertKind};
use crate::config::Config;
use std::path::{Path, PathBuf};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Everything a status report can say. Fields the machine cannot provide are left out of the text.
#[derive(Debug, Default, PartialEq)]
pub struct Snapshot {
    pub host: Option<String>,
    pub uptime_secs: Option<u64>,
    pub load: Option<String>,
    /// (used, total) in KiB
    pub memory_kib: Option<(u64, u64)>,
    /// (used, total) in KiB for `/`
    pub disk_kib: Option<(u64, u64)>,
    pub battery_percent: Option<u8>,
    pub plugged: Option<bool>,
}

pub fn parse_uptime(s: &str) -> Option<u64> {
    let v: f64 = s.split_whitespace().next()?.parse().ok()?;
    if v.is_finite() && v >= 0.0 { Some(v as u64) } else { None }
}

pub fn parse_loadavg(s: &str) -> Option<String> {
    let t: Vec<&str> = s.split_whitespace().take(3).collect();
    if t.len() == 3 { Some(t.join(" ")) } else { None }
}

/// Used = MemTotal - MemAvailable.
pub fn parse_meminfo(s: &str) -> Option<(u64, u64)> {
    let find = |key: &str| {
        s.lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|r| r.trim_start_matches(':').split_whitespace().next())
            .and_then(|n| n.parse::<u64>().ok())
    };
    let total = find("MemTotal")?;
    let avail = find("MemAvailable")?;
    Some((total.saturating_sub(avail), total))
}

/// Parses `df -Pk /` output.
pub fn parse_df(s: &str) -> Option<(u64, u64)> {
    let f: Vec<&str> = s.lines().nth(1)?.split_whitespace().collect();
    Some((f.get(2)?.parse().ok()?, f.get(1)?.parse().ok()?))
}

fn fmt_uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    if d > 0 { format!("{d}d {h}h {m}m") } else { format!("{h}h {m}m") }
}

fn gib(kib: u64) -> f64 {
    kib as f64 / 1024.0 / 1024.0
}

fn usage(used: u64, total: u64) -> String {
    let pct = if total == 0 { 0 } else { used * 100 / total };
    format!("{:.1} / {:.1} GiB used ({pct}%)", gib(used), gib(total))
}

pub fn format_report(s: &Snapshot) -> String {
    let mut lines: Vec<String> = vec![];
    if let Some(h) = &s.host {
        lines.push(format!("Host: {h}"));
    }
    if let Some(u) = s.uptime_secs {
        lines.push(format!("Uptime: {}", fmt_uptime(u)));
    }
    if let Some(l) = &s.load {
        lines.push(format!("Load: {l}"));
    }
    if let Some((used, total)) = s.memory_kib {
        lines.push(format!("Memory: {}", usage(used, total)));
    }
    if let Some((used, total)) = s.disk_kib {
        lines.push(format!("Disk /: {}", usage(used, total)));
    }
    let power = match s.plugged {
        Some(true) => "plugged in",
        Some(false) => "on battery",
        None => "",
    };
    match (s.battery_percent, power) {
        (Some(p), "") => lines.push(format!("Battery: {p}%")),
        (Some(p), w) => lines.push(format!("Battery: {p}% ({w})")),
        (None, "") => {}
        (None, w) => lines.push(format!("Power: {w}")),
    }
    lines.push(format!("Synnamon: {}", crate::update::current_version()));
    lines.join("\n")
}

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

pub fn collect(power_root: &Path) -> Snapshot {
    let disk = std::process::Command::new("df")
        .args(["-Pk", "/"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| parse_df(&String::from_utf8_lossy(&o.stdout)));
    Snapshot {
        host: read("/proc/sys/kernel/hostname").map(|h| h.trim().to_string()).filter(|h| !h.is_empty()),
        uptime_secs: read("/proc/uptime").and_then(|s| parse_uptime(&s)),
        load: read("/proc/loadavg").and_then(|s| parse_loadavg(&s)),
        memory_kib: read("/proc/meminfo").and_then(|s| parse_meminfo(&s)),
        disk_kib: disk,
        battery_percent: crate::battery::lowest(&crate::battery::read_batteries(power_root)).map(|r| r.percent),
        plugged: crate::power::read_plugged(power_root),
    }
}

/// Decides when a report is due. Disabled time does not count, so enabling starts a fresh interval
/// and the first report comes one interval after that, never immediately.
pub struct Timer {
    since: Instant,
}

impl Timer {
    pub fn new(now: Instant) -> Timer {
        Timer { since: now }
    }

    pub fn due(&mut self, enabled: bool, interval_mins: u64, now: Instant) -> bool {
        if !enabled {
            self.since = now;
            return false;
        }
        if now.duration_since(self.since) >= Duration::from_secs(interval_mins.max(1) * 60) {
            self.since = now;
            return true;
        }
        false
    }
}

pub fn spawn(cfg: Arc<RwLock<Config>>, tx: SyncSender<Alert>, power_root: PathBuf) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut timer = Timer::new(Instant::now());
        loop {
            // 1 s slices so config changes take effect promptly.
            std::thread::sleep(Duration::from_secs(1));
            let r = cfg.read().unwrap().report.clone();
            if timer.due(r.enabled, r.interval_mins, Instant::now()) {
                let body = format_report(&collect(&power_root));
                crate::daemon::send_alert(&tx, Alert { kind: AlertKind::Report, title: "Status report".into(), body });
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_files() {
        assert_eq!(parse_uptime("12345.67 9999.00\n"), Some(12345));
        assert_eq!(parse_uptime("garbage"), None);
        assert_eq!(parse_uptime("NaN 1"), None);
        assert_eq!(parse_loadavg("0.15 0.20 0.18 1/500 12345\n").as_deref(), Some("0.15 0.20 0.18"));
        assert_eq!(parse_loadavg("0.15"), None);
        let mem = "MemTotal:       16000000 kB\nMemFree: 1 kB\nMemAvailable:   10000000 kB\n";
        assert_eq!(parse_meminfo(mem), Some((6_000_000, 16_000_000)));
        assert_eq!(parse_meminfo("MemTotal: 5 kB\n"), None);
    }

    #[test]
    fn parses_df_output() {
        let df = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n/dev/nvme0n1p2   478000000 120000000 334000000      27% /\n";
        assert_eq!(parse_df(df), Some((120_000_000, 478_000_000)));
        assert_eq!(parse_df("Filesystem only\n"), None);
    }

    #[test]
    fn report_text_includes_everything_available() {
        let s = Snapshot {
            host: Some("laptop".into()),
            uptime_secs: Some(3 * 86_400 + 4 * 3_600 + 12 * 60),
            load: Some("0.15 0.20 0.18".into()),
            memory_kib: Some((8 * 1024 * 1024, 16 * 1024 * 1024)),
            disk_kib: Some((100 * 1024 * 1024, 400 * 1024 * 1024)),
            battery_percent: Some(73),
            plugged: Some(false),
        };
        let t = format_report(&s);
        assert!(t.contains("Host: laptop"), "{t}");
        assert!(t.contains("Uptime: 3d 4h 12m"), "{t}");
        assert!(t.contains("Load: 0.15 0.20 0.18"), "{t}");
        assert!(t.contains("Memory: 8.0 / 16.0 GiB used (50%)"), "{t}");
        assert!(t.contains("Disk /: 100.0 / 400.0 GiB used (25%)"), "{t}");
        assert!(t.contains("Battery: 73% (on battery)"), "{t}");
        assert!(t.contains("Synnamon: "), "{t}");
    }

    #[test]
    fn report_skips_what_the_machine_cannot_provide() {
        let t = format_report(&Snapshot::default());
        assert_eq!(t.lines().count(), 1, "{t}");
        assert!(t.starts_with("Synnamon: "));
        let desktop = Snapshot { plugged: Some(true), ..Default::default() };
        assert!(format_report(&desktop).contains("Power: plugged in"));
    }

    #[test]
    fn short_uptime_has_no_days() {
        assert_eq!(fmt_uptime(5 * 3_600 + 7 * 60), "5h 7m");
    }

    #[test]
    fn timer_fires_once_per_interval_and_not_at_start() {
        let t0 = Instant::now();
        let mut t = Timer::new(t0);
        assert!(!t.due(true, 60, t0));
        assert!(!t.due(true, 60, t0 + Duration::from_secs(3_599)));
        assert!(t.due(true, 60, t0 + Duration::from_secs(3_600)));
        assert!(!t.due(true, 60, t0 + Duration::from_secs(3_601)));
        assert!(t.due(true, 60, t0 + Duration::from_secs(7_200)));
    }

    #[test]
    fn timer_never_fires_while_disabled_and_restarts_when_enabled() {
        let t0 = Instant::now();
        let mut t = Timer::new(t0);
        assert!(!t.due(false, 60, t0 + Duration::from_secs(10_000)));
        // Enabled right after being disabled: a full interval must pass first.
        assert!(!t.due(true, 60, t0 + Duration::from_secs(10_001)));
        assert!(t.due(true, 60, t0 + Duration::from_secs(10_000 + 3_600)));
    }

    #[test]
    fn timer_honours_a_shortened_interval() {
        let t0 = Instant::now();
        let mut t = Timer::new(t0);
        assert!(t.due(true, 5, t0 + Duration::from_secs(300)));
    }
}
