//! Discord Rich Presence's connection, where the platform has one.

use std::sync::mpsc;

#[cfg_attr(
    any(target_os = "android", target_os = "ios"),
    allow(dead_code, reason = "read only by the desktop Rich Presence worker")
)]
pub(crate) const GET_CALIBRAW_URL: &str = env!("CARGO_PKG_REPOSITORY");

#[cfg_attr(
    any(target_os = "android", target_os = "ios"),
    allow(dead_code, reason = "read only by the desktop Rich Presence worker")
)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum PresenceActivity {
    Browsing,
    Editing { started_at: i64 },
}

#[cfg_attr(
    any(target_os = "android", target_os = "ios"),
    allow(dead_code, reason = "read only by the desktop Rich Presence worker")
)]
pub(crate) enum PresenceCommand {
    Set(PresenceActivity),
    Shutdown,
}

/// Whether this build can reach Discord: not on Android, and not inside
/// Flatpak, which is not granted Discord's IPC socket.
pub(crate) fn presence_supported() -> bool {
    imp::presence_supported()
}

/// Keeps the Discord activity at the last `Set` until `Shutdown`, retrying
/// while Discord is not running. Runs on its own thread.
pub(crate) fn run_presence_worker(application_id: &str, receiver: mpsc::Receiver<PresenceCommand>) {
    imp::run_presence_worker(application_id, receiver);
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use super::desktop::discord as imp;

#[cfg(any(target_os = "android", target_os = "ios"))]
mod imp {
    use super::PresenceCommand;
    use std::sync::mpsc;

    pub(super) fn presence_supported() -> bool {
        false
    }

    pub(super) fn run_presence_worker(
        _application_id: &str,
        _receiver: mpsc::Receiver<PresenceCommand>,
    ) {
    }
}
