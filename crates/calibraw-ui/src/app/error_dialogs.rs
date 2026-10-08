//! Error popups. A failure the user must know about is shown in a dialog as
//! well as the status line, so it is never visible only in the logs.

use super::*;

/// Errors waiting beyond this many are dropped; the status line still shows the latest.
const MAX_QUEUED_ERROR_DIALOGS: usize = 8;
/// How long a dismissed error stays quiet when it recurs, so an automatic
/// retry that fails the same way does not reopen the dialog at once.
const REPEAT_QUIET_PERIOD: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
struct ErrorDialog {
    title: String,
    message: String,
}

/// Error dialogs shown one at a time, oldest first.
#[derive(Debug, Default)]
pub(crate) struct ErrorDialogQueue {
    dialogs: VecDeque<ErrorDialog>,
    last_dismissed: Option<(ErrorDialog, Instant)>,
}

impl ErrorDialogQueue {
    /// Queues an error unless the same one is already waiting or was just
    /// dismissed, so a failure that repeats shows once.
    fn push(&mut self, title: &str, message: String, now: Instant) {
        let dialog = ErrorDialog {
            title: title.to_owned(),
            message,
        };
        let recently_dismissed = self.last_dismissed.as_ref().is_some_and(|(last, at)| {
            *last == dialog && now.saturating_duration_since(*at) < REPEAT_QUIET_PERIOD
        });
        if recently_dismissed
            || self.dialogs.contains(&dialog)
            || self.dialogs.len() >= MAX_QUEUED_ERROR_DIALOGS
        {
            return;
        }
        self.dialogs.push_back(dialog);
    }

    pub(crate) fn is_open(&self) -> bool {
        !self.dialogs.is_empty()
    }

    /// Closes the visible dialog and shows the next one, if any.
    pub(crate) fn dismiss_current(&mut self) {
        self.dismiss_current_at(Instant::now());
    }

    fn dismiss_current_at(&mut self, now: Instant) {
        if let Some(dialog) = self.dialogs.pop_front() {
            self.last_dismissed = Some((dialog, now));
        }
    }
}

impl CalibRawApp {
    /// Reports a failure: shows `message` in an error dialog titled `title`,
    /// keeps it in the status line, and logs it.
    pub(crate) fn report_error(&mut self, title: &str, message: impl Into<String>) {
        let message = message.into();
        log::error!("{title}: {message}");
        self.ui.notice = Some(message.clone());
        self.ui.error_dialogs.push(title, message, Instant::now());
        self.egui_ctx.request_repaint();
    }

    pub(in crate::app) fn show_error_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.ui.error_dialogs.dialogs.front().cloned() else {
            return;
        };
        let remaining = self.ui.error_dialogs.dialogs.len() - 1;
        let mut close = false;
        moduwu_design::dialog_window(&dialog.title, ctx, moduwu_design::DIALOG_WIDTH_DEFAULT)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(&dialog.message);
                if remaining > 0 {
                    ui.add_space(moduwu_design::SPACE_XS);
                    ui.label(
                        egui::RichText::new(if remaining == 1 {
                            "1 more error follows.".to_owned()
                        } else {
                            format!("{remaining} more errors follow.")
                        })
                        .small()
                        .color(ui.visuals().weak_text_color()),
                    );
                }
                moduwu_design::dialog_button_row(ui, |ui| {
                    close |= moduwu_design::secondary_button(ui, "Close").clicked();
                });
                if !close
                    && moduwu_design::dialog_keyboard_action(
                        ui,
                        moduwu_design::DialogKeyboard::CLOSE_ONLY,
                        false,
                    ) == moduwu_design::DialogAction::Cancel
                {
                    close = true;
                }
            });
        if close {
            self.ui.error_dialogs.dismiss_current();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_errors_queue_once() {
        let now = Instant::now();
        let mut queue = ErrorDialogQueue::default();
        queue.push("Export failed", "out of memory".to_owned(), now);
        queue.push("Export failed", "out of memory".to_owned(), now);
        queue.push("Export failed", "disk full".to_owned(), now);
        assert_eq!(queue.dialogs.len(), 2);
    }

    #[test]
    fn a_dismissed_error_stays_quiet_briefly_then_shows_again() {
        let now = Instant::now();
        let mut queue = ErrorDialogQueue::default();
        queue.push("Preview failed", "no GPU".to_owned(), now);
        queue.dismiss_current_at(now);
        assert!(!queue.is_open());

        queue.push(
            "Preview failed",
            "no GPU".to_owned(),
            now + Duration::from_secs(1),
        );
        assert!(!queue.is_open());
        queue.push(
            "Export failed",
            "disk full".to_owned(),
            now + Duration::from_secs(1),
        );
        assert!(queue.is_open());
        queue.dismiss_current_at(now + Duration::from_secs(1));

        queue.push(
            "Preview failed",
            "no GPU".to_owned(),
            now + REPEAT_QUIET_PERIOD + Duration::from_secs(1),
        );
        assert!(queue.is_open());
    }

    #[test]
    fn the_queue_is_bounded() {
        let now = Instant::now();
        let mut queue = ErrorDialogQueue::default();
        for index in 0..MAX_QUEUED_ERROR_DIALOGS + 3 {
            queue.push("Error", format!("failure {index}"), now);
        }
        assert_eq!(queue.dialogs.len(), MAX_QUEUED_ERROR_DIALOGS);
        assert_eq!(queue.dialogs[0].message, "failure 0");
    }
}
