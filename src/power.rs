use crate::alert::{Alert, AlertKind};
use crate::config::Config;
use std::path::{Path, PathBuf};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

/// Sysfs is cheap to read, and plug events should be reported within seconds.
const POLL: Duration = Duration::from_secs(2);

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// Whether external power is connected. Uses the AC / USB-C supplies when the machine has any,
/// otherwise the battery status. `None` when the machine reports neither (e.g. a VM).
pub fn read_plugged(root: &Path) -> Option<bool> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut sources: Vec<bool> = vec![];
    let mut batteries: Vec<bool> = vec![];
    for e in entries.flatten() {
        let dir = e.path();
        // Peripherals (wireless mice, headsets, phones) report scope=Device.
        if read_trim(&dir.join("scope")).as_deref() == Some("Device") {
            continue;
        }
        let Some(ty) = read_trim(&dir.join("type")) else { continue };
        if ty == "Mains" || ty.starts_with("USB") {
            if let Some(online) = read_trim(&dir.join("online")) {
                sources.push(online == "1");
            }
        } else if ty == "Battery" {
            if let Some(status) = read_trim(&dir.join("status")) {
                batteries.push(matches!(status.as_str(), "Charging" | "Full" | "Not charging"));
            }
        }
    }
    let list = if sources.is_empty() { batteries } else { sources };
    if list.is_empty() { None } else { Some(list.contains(&true)) }
}

/// Turns raw readings into alerts. The first reading is only a baseline, and a change must be seen
/// on two consecutive readings, so a loose connector flickering for an instant does not alert.
pub struct Tracker {
    state: Option<bool>,
    pending: Option<bool>,
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker { state: None, pending: None }
    }

    pub fn update(&mut self, plugged: bool, battery_percent: Option<u8>) -> Option<Alert> {
        let Some(current) = self.state else {
            self.state = Some(plugged);
            return None;
        };
        if plugged == current {
            self.pending = None;
            return None;
        }
        if self.pending != Some(plugged) {
            self.pending = Some(plugged);
            return None;
        }
        self.state = Some(plugged);
        self.pending = None;
        let level = battery_percent.map(|p| format!(" (battery {p}%)")).unwrap_or_default();
        Some(if plugged {
            Alert { kind: AlertKind::Power, title: "Power plugged in".into(), body: format!("External power connected{level}") }
        } else {
            Alert {
                kind: AlertKind::Power,
                title: "Power unplugged".into(),
                body: format!("External power disconnected, running on battery{level}"),
            }
        })
    }
}

pub fn spawn(cfg: Arc<RwLock<Config>>, tx: SyncSender<Alert>, root: PathBuf) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut tracker = Tracker::new();
        let mut warned_none = false;
        loop {
            if cfg.read().unwrap().power.enabled {
                match read_plugged(&root) {
                    Some(plugged) => {
                        let percent = crate::battery::lowest(&crate::battery::read_batteries(&root)).map(|r| r.percent);
                        if let Some(a) = tracker.update(plugged, percent) {
                            crate::daemon::send_alert(&tx, a);
                        }
                    }
                    None if !warned_none => {
                        eprintln!("power: no AC adapter or battery found, monitor idle");
                        warned_none = true;
                    }
                    None => {}
                }
            } else {
                // Re-baseline when re-enabled, so a change made while disabled is not reported.
                tracker = Tracker::new();
            }
            std::thread::sleep(POLL);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn supply(root: &Path, name: &str, ty: &str, key: &str, value: &str) {
        let d = root.join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("type"), format!("{ty}\n")).unwrap();
        fs::write(d.join(key), format!("{value}\n")).unwrap();
    }

    #[test]
    fn mains_online_means_plugged() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "AC", "Mains", "online", "1");
        supply(d.path(), "BAT0", "Battery", "status", "Discharging");
        assert_eq!(read_plugged(d.path()), Some(true));
    }

    #[test]
    fn mains_offline_means_unplugged_even_if_battery_says_full() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "AC", "Mains", "online", "0");
        supply(d.path(), "BAT0", "Battery", "status", "Full");
        assert_eq!(read_plugged(d.path()), Some(false));
    }

    #[test]
    fn usb_c_charger_counts_as_power() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "ucsi-source-psy-USBC000_001", "USB", "online", "1");
        supply(d.path(), "BAT0", "Battery", "status", "Discharging");
        assert_eq!(read_plugged(d.path()), Some(true));
    }

    #[test]
    fn falls_back_to_battery_status_without_an_adapter_entry() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "BAT0", "Battery", "status", "Charging");
        assert_eq!(read_plugged(d.path()), Some(true));
        supply(d.path(), "BAT0", "Battery", "status", "Discharging");
        assert_eq!(read_plugged(d.path()), Some(false));
    }

    #[test]
    fn peripheral_supplies_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "AC", "Mains", "online", "0");
        supply(d.path(), "dongle", "USB", "online", "1");
        fs::write(d.path().join("dongle/scope"), "Device\n").unwrap();
        assert_eq!(read_plugged(d.path()), Some(false));
    }

    #[test]
    fn nothing_to_read_is_none() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read_plugged(d.path()), None);
        assert_eq!(read_plugged(&d.path().join("missing")), None);
    }

    #[test]
    fn first_reading_is_a_silent_baseline() {
        assert!(Tracker::new().update(true, Some(80)).is_none());
        assert!(Tracker::new().update(false, Some(80)).is_none());
    }

    #[test]
    fn unplug_alerts_after_two_consecutive_readings() {
        let mut t = Tracker::new();
        t.update(true, Some(80));
        assert!(t.update(false, Some(79)).is_none());
        let a = t.update(false, Some(79)).unwrap();
        assert_eq!(a.kind, AlertKind::Power);
        assert_eq!(a.title, "Power unplugged");
        assert!(a.body.contains("79%"), "{}", a.body);
    }

    #[test]
    fn plug_in_alerts_and_steady_state_is_silent() {
        let mut t = Tracker::new();
        t.update(false, None);
        t.update(true, None);
        let a = t.update(true, None).unwrap();
        assert_eq!(a.title, "Power plugged in");
        assert!(!a.body.contains('%'), "{}", a.body);
        assert!(t.update(true, None).is_none());
        assert!(t.update(true, None).is_none());
    }

    #[test]
    fn a_one_reading_flicker_does_not_alert() {
        let mut t = Tracker::new();
        t.update(true, None);
        assert!(t.update(false, None).is_none());
        assert!(t.update(true, None).is_none());
        assert!(t.update(true, None).is_none());
    }
}
