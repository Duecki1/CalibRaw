use super::AppTab;
use crate::i18n::{tr, tr_fmt};
use crate::platform::discord::{
    presence_supported, run_presence_worker, PresenceActivity, PresenceCommand,
};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

const APPLICATION_ID_ENV: &str = "CALIBRAW_DISCORD_APPLICATION_ID";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PresenceContext {
    Browsing,
    Editing { document_id: u64 },
}

pub(super) struct DiscordPresence {
    supported: bool,
    application_id: Option<String>,
    enabled: bool,
    sender: Option<mpsc::Sender<PresenceCommand>>,
    current_context: Option<PresenceContext>,
    last_non_settings_context: Option<PresenceContext>,
}

impl DiscordPresence {
    pub(super) fn new(enabled: bool) -> Self {
        let supported = presence_supported();
        // An unsupported build never connects, even if an older version saved
        // the setting as enabled.
        let application_id = configured_application_id().filter(|_| supported);
        let enabled = enabled && application_id.is_some();
        if enabled {
            log::info!("Discord Rich Presence is enabled");
        }
        Self {
            supported,
            application_id,
            enabled,
            sender: None,
            current_context: None,
            last_non_settings_context: None,
        }
    }

    pub(super) fn is_supported(&self) -> bool {
        self.supported
    }

    pub(super) fn is_configured(&self) -> bool {
        self.application_id.is_some()
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) -> Result<(), String> {
        if enabled && !self.supported {
            return Err(
                tr("Discord Rich Presence is unavailable in the Flatpak build.").to_owned(),
            );
        }
        if enabled && self.application_id.is_none() {
            return Err(tr_fmt(
                "Discord Rich Presence is unavailable because this build has no {variable}.",
                &[("variable", &APPLICATION_ID_ENV)],
            ));
        }
        if self.enabled == enabled {
            return Ok(());
        }

        self.enabled = enabled;
        self.current_context = None;
        if !enabled {
            self.shutdown_worker();
        }
        Ok(())
    }

    pub(super) fn sync(&mut self, tab: AppTab, document_id: Option<u64>) {
        let context = match tab {
            AppTab::Library => Some(PresenceContext::Browsing),
            AppTab::Develop => {
                document_id.map(|document_id| PresenceContext::Editing { document_id })
            }
            AppTab::Settings => self.last_non_settings_context,
        };

        if tab != AppTab::Settings {
            if let Some(context) = context {
                self.last_non_settings_context = Some(context);
            }
        }
        if !self.enabled || context.is_none() || self.current_context == context {
            return;
        }

        let context = context.expect("presence context was checked above");
        let activity = match context {
            PresenceContext::Browsing => PresenceActivity::Browsing,
            PresenceContext::Editing { .. } => PresenceActivity::Editing {
                started_at: unix_timestamp_seconds(),
            },
        };
        if self.send(PresenceCommand::Set(activity)) {
            self.current_context = Some(context);
        }
    }

    pub(super) fn shutdown(&mut self) {
        self.enabled = false;
        self.current_context = None;
        self.shutdown_worker();
    }

    fn send(&mut self, command: PresenceCommand) -> bool {
        if self.sender.is_none() && !self.start_worker() {
            return false;
        }
        let Some(sender) = self.sender.as_ref() else {
            return false;
        };
        if sender.send(command).is_ok() {
            return true;
        }

        self.sender = None;
        false
    }

    fn start_worker(&mut self) -> bool {
        let Some(application_id) = self.application_id.clone() else {
            return false;
        };
        let (sender, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("calibraw-discord-presence".to_owned())
            .spawn(move || run_presence_worker(&application_id, receiver))
        {
            Ok(_) => {
                self.sender = Some(sender);
                true
            }
            Err(error) => {
                log::warn!("could not start Discord Rich Presence worker: {error}");
                false
            }
        }
    }

    fn shutdown_worker(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(PresenceCommand::Shutdown);
        }
    }
}

impl Drop for DiscordPresence {
    fn drop(&mut self) {
        self.shutdown_worker();
    }
}

fn configured_application_id() -> Option<String> {
    option_env!("CALIBRAW_DISCORD_APPLICATION_ID")
        .map(str::to_owned)
        .or_else(|| std::env::var(APPLICATION_ID_ENV).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| value.parse::<u64>().is_ok_and(|value| value != 0))
}

fn unix_timestamp_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or_default()
}
