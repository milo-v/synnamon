use crate::alert::{self, Alert};
use crate::config::Config;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

/// Never blocks monitors: if the queue is full the alert is dropped and logged.
pub fn send_alert(tx: &SyncSender<Alert>, a: Alert) {
    if tx.try_send(a).is_err() {
        eprintln!("alert queue full or closed; alert dropped");
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Reload the config if the file's mtime changed. A bad file keeps the old config.
pub fn reload_if_changed(path: &Path, last: &mut Option<SystemTime>, cfg: &RwLock<Config>) -> bool {
    let now = mtime(path);
    if now == *last {
        return false;
    }
    *last = now; // remember it even on failure so a bad file is reported once
    match Config::load(path) {
        Ok(new) => {
            *cfg.write().unwrap() = new;
            eprintln!("config reloaded");
            true
        }
        Err(e) => {
            eprintln!("config reload failed, keeping previous config: {e:#}");
            false
        }
    }
}

pub fn run(config_path: PathBuf) -> Result<()> {
    let cfg = Arc::new(RwLock::new(Config::load(&config_path)?));
    let mut last = mtime(&config_path);
    let (tx, rx) = sync_channel::<Alert>(64);

    alert::spawn_sender(rx, cfg.clone());
    crate::battery::spawn(cfg.clone(), tx.clone(), PathBuf::from(crate::battery::SYSFS_ROOT));
    crate::logins::spawn(cfg.clone(), tx.clone());
    eprintln!("synnamon running (config: {})", config_path.display());

    loop {
        std::thread::sleep(Duration::from_secs(2));
        reload_if_changed(&config_path, &mut last, &cfg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alert::AlertKind;
    use std::fs::{self, File};

    fn write_at(p: &Path, text: &str, secs: u64) {
        fs::write(p, text).unwrap();
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs);
        File::options().write(true).open(p).unwrap().set_modified(t).unwrap();
    }

    fn setup(text: &str) -> (tempfile::TempDir, PathBuf, RwLock<Config>) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        write_at(&p, text, 0);
        let cfg = RwLock::new(Config::load(&p).unwrap());
        (d, p, cfg)
    }

    #[test]
    fn reload_applies_valid_change() {
        let (_d, p, cfg) = setup("[battery]\nthreshold = 20\n");
        let mut last = mtime(&p);
        write_at(&p, "[battery]\nthreshold = 35\n", 10);
        assert!(reload_if_changed(&p, &mut last, &cfg));
        assert_eq!(cfg.read().unwrap().battery.threshold, 35);
    }

    #[test]
    fn reload_keeps_old_config_on_invalid_toml() {
        let (_d, p, cfg) = setup("[battery]\nthreshold = 20\n");
        let mut last = mtime(&p);
        write_at(&p, "[battery\nbroken", 10);
        assert!(!reload_if_changed(&p, &mut last, &cfg));
        assert_eq!(cfg.read().unwrap().battery.threshold, 20);
    }

    #[test]
    fn reload_keeps_old_config_on_threshold_101() {
        let (_d, p, cfg) = setup("[battery]\nthreshold = 20\n");
        let mut last = mtime(&p);
        write_at(&p, "[battery]\nthreshold = 101\n", 10);
        assert!(!reload_if_changed(&p, &mut last, &cfg));
        assert_eq!(cfg.read().unwrap().battery.threshold, 20);
    }

    #[test]
    fn reload_noop_when_mtime_unchanged() {
        let (_d, p, cfg) = setup("[battery]\nthreshold = 20\n");
        let mut last = mtime(&p);
        assert!(!reload_if_changed(&p, &mut last, &cfg));
    }

    #[test]
    fn send_alert_does_not_block_when_full() {
        let (tx, _rx) = sync_channel::<Alert>(1);
        for _ in 0..3 {
            send_alert(&tx, Alert { kind: AlertKind::Test, title: "t".into(), body: "b".into() });
        }
    }
}
