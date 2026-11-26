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

/// Whether external power is connected, or `None` when the machine reports neither an adapter nor a
/// battery (e.g. a VM).
///
/// The battery status is trusted first, because some laptops (ThinkPad E490) leave `AC/online` at 1
/// when unplugged and report `Not charging` instead of `Discharging`. `Not charging` is ambiguous on
/// machines with a charge limit (plugged in, held at the limit), so there, and when the status is
/// `Unknown`, the AC / USB-C supplies decide.
pub fn read_plugged(root: &Path) -> Option<bool> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut sources: Vec<bool> = vec![];
    let (mut charging, mut discharging, mut not_charging, mut held_at_limit) = (false, false, false, false);
    let mut has_battery = false;
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
            let Some(status) = read_trim(&dir.join("status")) else { continue };
            has_battery = true;
            match status.as_str() {
                "Charging" | "Full" => charging = true,
                "Discharging" => discharging = true,
                "Not charging" => {
                    not_charging = true;
                    let level: Option<u8> = read_trim(&dir.join("capacity")).and_then(|s| s.parse().ok());
                    let limit: Option<u8> = ["charge_control_end_threshold", "charge_stop_threshold"]
                        .iter()
                        .find_map(|f| read_trim(&dir.join(f)).and_then(|s| s.parse().ok()));
                    if matches!((level, limit), (Some(l), Some(t)) if l >= t) {
                        held_at_limit = true;
                    }
                }
                _ => {}
            }
        }
    }
    let from_adapter = || if sources.is_empty() { None } else { Some(sources.contains(&true)) };
    if has_battery {
        if charging {
            return Some(true);
        }
        if discharging {
            return Some(false);
        }
        if not_charging && !held_at_limit {
            return Some(false);
        }
    }
    from_adapter()
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
        let mut logged_baseline = false;
        loop {
            if cfg.read().unwrap().power.enabled {
                match read_plugged(&root) {
                    Some(plugged) => {
                        let percent = crate::battery::lowest(&crate::battery::read_batteries(&root)).map(|r| r.percent);
                        if !logged_baseline {
                            eprintln!("power: external power is {}", if plugged { "connected" } else { "disconnected" });
                            logged_baseline = true;
                        }
                        if let Some(a) = tracker.update(plugged, percent) {
                            eprintln!("power: {}", a.title);
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
    fn adapter_alone_decides_when_there_is_no_battery() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "AC", "Mains", "online", "1");
        assert_eq!(read_plugged(d.path()), Some(true));
        supply(d.path(), "AC", "Mains", "online", "0");
        assert_eq!(read_plugged(d.path()), Some(false));
    }

    #[test]
    fn thinkpad_e490_not_charging_with_stuck_ac_means_unplugged() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "AC", "Mains", "online", "1");
        supply(d.path(), "BAT0", "Battery", "status", "Not charging");
        assert_eq!(read_plugged(d.path()), Some(false));
        supply(d.path(), "BAT0", "Battery", "status", "Charging");
        assert_eq!(read_plugged(d.path()), Some(true));
    }

    #[test]
    fn battery_status_wins_over_a_stuck_adapter() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "AC", "Mains", "online", "1");
        supply(d.path(), "BAT0", "Battery", "status", "Discharging");
        assert_eq!(read_plugged(d.path()), Some(false));
        supply(d.path(), "AC", "Mains", "online", "0");
        supply(d.path(), "BAT0", "Battery", "status", "Full");
        assert_eq!(read_plugged(d.path()), Some(true));
    }

    #[test]
    fn not_charging_at_the_charge_limit_defers_to_the_adapter() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "AC", "Mains", "online", "1");
        supply(d.path(), "BAT0", "Battery", "status", "Not charging");
        fs::write(d.path().join("BAT0/capacity"), "80\n").unwrap();
        fs::write(d.path().join("BAT0/charge_control_end_threshold"), "80\n").unwrap();
        assert_eq!(read_plugged(d.path()), Some(true));
        supply(d.path(), "AC", "Mains", "online", "0");
        assert_eq!(read_plugged(d.path()), Some(false));
    }

    #[test]
    fn unknown_battery_status_defers_to_the_adapter() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "AC", "Mains", "online", "1");
        supply(d.path(), "BAT0", "Battery", "status", "Unknown");
        assert_eq!(read_plugged(d.path()), Some(true));
    }

    #[test]
    fn usb_c_charger_counts_as_power() {
        let d = tempfile::tempdir().unwrap();
        supply(d.path(), "ucsi-source-psy-USBC000_001", "USB", "online", "1");
        supply(d.path(), "BAT0", "Battery", "status", "Unknown");
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
