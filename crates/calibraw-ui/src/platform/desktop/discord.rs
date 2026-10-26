//! The Discord IPC connection behind Rich Presence.

use super::super::discord::{PresenceActivity, PresenceCommand, GET_CALIBRAW_URL};
use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use std::sync::mpsc;
use std::time::Duration;

const DISCONNECTED_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const CONNECTED_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// The Flatpak is not granted Discord's IPC socket, so it leaves out Rich
/// Presence along with its setup step and setting.
pub(in crate::platform) fn presence_supported() -> bool {
    !super::portal::is_flatpak()
}

pub(in crate::platform) fn run_presence_worker(
    application_id: &str,
    receiver: mpsc::Receiver<PresenceCommand>,
) {
    let mut client = None;
    let mut desired_activity = None;

    loop {
        let timeout = if client.is_some() {
            CONNECTED_REFRESH_INTERVAL
        } else {
            DISCONNECTED_RETRY_INTERVAL
        };
        match receiver.recv_timeout(timeout) {
            Ok(PresenceCommand::Set(activity)) => desired_activity = Some(activity),
            Ok(PresenceCommand::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                clear_and_close(&mut client);
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        let Some(activity) = desired_activity else {
            continue;
        };
        if client.is_none() {
            let mut next_client = DiscordIpcClient::new(application_id);
            match next_client.connect() {
                Ok(()) => client = Some(next_client),
                Err(error) => {
                    log::debug!("Discord Rich Presence is waiting for Discord: {error}");
                    continue;
                }
            }
        }

        let result = client
            .as_mut()
            .expect("Discord client was connected above")
            .set_activity(activity_payload(activity));
        if let Err(error) = result {
            log::debug!("Discord Rich Presence update failed; reconnecting: {error}");
            clear_and_close(&mut client);
        }
    }
}

fn activity_payload(presence: PresenceActivity) -> activity::Activity<'static> {
    let payload = activity::Activity::new()
        .activity_type(activity::ActivityType::Playing)
        .buttons(vec![activity::Button::new(
            "Get CalibRaw",
            GET_CALIBRAW_URL,
        )]);
    match presence {
        PresenceActivity::Browsing => payload.details("Browsing Photos"),
        PresenceActivity::Editing { started_at } => payload
            .details("Editing a Picture")
            .timestamps(activity::Timestamps::new().start(started_at)),
    }
}

fn clear_and_close(client: &mut Option<DiscordIpcClient>) {
    if let Some(mut client) = client.take() {
        let _ = client.clear_activity();
        let _ = client.close();
    }
}
