use crate::alert::{Alert, AlertKind};
use crate::config::Config;
use std::path::{Path, PathBuf};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

pub const SYSFS_ROOT: &str = "/sys/class/power_supply";
const HYSTERESIS: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub percent: u8,
    pub charging: bool,
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

pub fn read_batteries(root: &Path) -> Vec<Reading> {
    let Ok(entries) = std::fs::read_dir(root) else { return vec![] };
    let mut out = vec![];
    for e in entries.flatten() {
        let dir = e.path();
        if read_trim(&dir.join("type")).as_deref() != Some("Battery") {
            continue;
        }
        let Some(percent) = read_trim(&dir.join("capacity")).and_then(|s| s.parse::<u8>().ok()) else {
            continue;
        };
        let charging = matches!(read_trim(&dir.join("status")).as_deref(), Some("Charging") | Some("Full"));
        out.push(Reading { percent: percent.min(100), charging });
    }
    out
}

pub fn lowest(readings: &[Reading]) -> Option<Reading> {
    let percent = readings.iter().map(|r| r.percent).min()?;
    let charging = readings.iter().all(|r| r.charging);
    Some(Reading { percent, charging })
}

pub struct Tracker {
    armed: bool,
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker { armed: true }
    }

    pub fn update(&mut self, r: Reading, threshold: u8) -> Option<Alert> {
        if r.charging || r.percent >= threshold.saturating_add(HYSTERESIS) {
            self.armed = true;
        }
        if self.armed && r.percent < threshold && !r.charging {
            self.armed = false;
            return Some(Alert {
                kind: AlertKind::Battery,
                title: "Low battery".into(),
                body: format!("Battery at {}% (threshold {}%), not charging", r.percent, threshold),
            });
        }
        None
    }
}

pub fn spawn(cfg: Arc<RwLock<Config>>, tx: SyncSender<Alert>, root: PathBuf) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut tracker = Tracker::new();
        let mut warned_none = false;
        loop {
            let b = cfg.read().unwrap().battery.clone();
            if b.enabled {
                match lowest(&read_batteries(&root)) {
                    Some(r) => {
                        if let Some(a) = tracker.update(r, b.threshold) {
                            crate::daemon::send_alert(&tx, a);
                        }
                    }
                    None if !warned_none => {
                        eprintln!("battery: no battery found, monitor idle");
                        warned_none = true;
                    }
                    None => {}
                }
            }
            // Sleep in 1 s slices so config reloads take effect promptly.
            let mut waited = 0;
            while waited < cfg.read().unwrap().battery.poll_secs {
                std::thread::sleep(Duration::from_secs(1));
                waited += 1;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn r(percent: u8, charging: bool) -> Reading {
        Reading { percent, charging }
    }

    #[test]
    fn no_alert_at_exactly_threshold() {
        assert!(Tracker::new().update(r(20, false), 20).is_none());
    }

    #[test]
    fn alerts_once_when_below() {
        let a = Tracker::new().update(r(19, false), 20).unwrap();
        assert_eq!(a.kind, AlertKind::Battery);
        assert_eq!(a.body, "Battery at 19% (threshold 20%), not charging");
    }

    #[test]
    fn no_repeat_while_staying_low() {
        let mut t = Tracker::new();
        assert!(t.update(r(19, false), 20).is_some());
        assert!(t.update(r(18, false), 20).is_none());
        assert!(t.update(r(10, false), 20).is_none());
    }

    #[test]
    fn no_alert_when_low_but_charging() {
        assert!(Tracker::new().update(r(5, true), 20).is_none());
    }

    #[test]
    fn rearms_after_recovering_threshold_plus_5() {
        let mut t = Tracker::new();
        assert!(t.update(r(19, false), 20).is_some());
        assert!(t.update(r(22, false), 20).is_none());
        assert!(t.update(r(25, false), 20).is_none());
        assert!(t.update(r(19, false), 20).is_some());
    }

    #[test]
    fn flapping_between_19_and_21_alerts_once() {
        let mut t = Tracker::new();
        let n = [19, 21, 19, 21, 19].iter().filter(|p| t.update(r(**p, false), 20).is_some()).count();
        assert_eq!(n, 1);
    }

    #[test]
    fn charging_rearms() {
        let mut t = Tracker::new();
        assert!(t.update(r(15, false), 20).is_some());
        assert!(t.update(r(15, true), 20).is_none());
        assert!(t.update(r(14, false), 20).is_some());
    }

    fn supply(root: &std::path::Path, name: &str, ty: &str, cap: Option<&str>, status: &str) {
        let d = root.join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("type"), format!("{ty}\n")).unwrap();
        fs::write(d.join("status"), format!("{status}\n")).unwrap();
        if let Some(c) = cap {
            fs::write(d.join("capacity"), format!("{c}\n")).unwrap();
        }
    }

    #[test]
    fn lowest_of_two_batteries_is_used() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "BAT0", "Battery", Some("80"), "Discharging");
        supply(d.path(), "BAT1", "Battery", Some("12"), "Discharging");
        let l = lowest(&read_batteries(d.path())).unwrap();
        assert_eq!(l.percent, 12);
        assert!(!l.charging);
    }

    #[test]
    fn missing_capacity_is_skipped() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "BAT0", "Battery", None, "Discharging");
        supply(d.path(), "BAT1", "Battery", Some("garbage"), "Discharging");
        assert!(read_batteries(d.path()).is_empty());
    }

    #[test]
    fn non_battery_supply_ignored() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "AC", "Mains", Some("100"), "Unknown");
        assert!(read_batteries(d.path()).is_empty());
    }

    #[test]
    fn no_batteries_returns_empty() {
        let d = tempfile::tempdir().unwrap();
        assert!(read_batteries(d.path()).is_empty());
        assert!(read_batteries(&d.path().join("missing")).is_empty());
    }

    #[test]
    fn charging_only_if_all_charging() {
        let l = lowest(&[r(30, true), r(50, false)]).unwrap();
        assert!(!l.charging);
        let l = lowest(&[r(30, true), r(50, true)]).unwrap();
        assert!(l.charging);
    }
}
