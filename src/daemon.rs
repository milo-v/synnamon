use crate::alert::Alert;
use std::sync::mpsc::SyncSender;

/// Never blocks monitors: if the queue is full the alert is dropped and logged.
pub fn send_alert(tx: &SyncSender<Alert>, a: Alert) {
    if tx.try_send(a).is_err() {
        eprintln!("alert queue full or closed; alert dropped");
    }
}
