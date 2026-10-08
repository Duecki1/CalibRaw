//! Error popups. A failure the user must know about is shown in a dialog as
//! well as the status line, so it is never visible only in the logs.

use super::*;

/// Errors waiting beyond this many are dropped; the status line still shows the latest.
const MAX_QUEUED_ERROR_DIALOGS: usize = 8;
/// How long a dismissed error stays quiet when it recurs, so an automatic
/// retry that fails the same way does not reopen the dialog at once.
const REPEAT_QUIET_PERIOD: Duration = Duration::from_secs(10);

/// What failed; it titles the error dialog. Add a kind rather than reusing
/// one whose title would mislead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    Export,
    ExportUnavailable,
    BatchExport,
    #[cfg(target_os = "android")]
    Share,
    EditReplay,
    Preview,
    Sampling,
    LensCorrection,
    OpenPhoto,
    #[cfg(target_os = "android")]
    Import,
    PhotoOpenedWithProblems,
    CameraProfile,
    Presets,
    AiDenoise,
    AiDenoiseNotSaved,
    AiMask,
    AiMaskRefresh,
    AiUpdate,
    #[cfg(not(target_os = "android"))]
    AiRuntime,
    GpuMemory,
    Retouch,
    Straighten,
    Settings,
    EditNotApplied,
    #[cfg(target_os = "android")]
    LibraryAdjustments,
}

impl ErrorKind {
    fn title(self) -> &'static str {
        match self {
            Self::Export => "Export failed",
            Self::ExportUnavailable => "Export unavailable",
            Self::BatchExport => "Batch export failed",
            #[cfg(target_os = "android")]
            Self::Share => "Sharing failed",
            Self::EditReplay => "Edit replay failed",
            Self::Preview => "Preview failed",
            Self::Sampling => "Sampling failed",
            Self::LensCorrection => "Lens correction failed",
            Self::OpenPhoto => "Could not open photo",
            #[cfg(target_os = "android")]
            Self::Import => "Import failed",
            Self::PhotoOpenedWithProblems => "Photo opened with problems",
            Self::CameraProfile => "Camera profile failed",
            Self::Presets => "Presets failed",
            Self::AiDenoise => "AI denoise failed",
            Self::AiDenoiseNotSaved => "AI denoise not saved",
            Self::AiMask => "AI mask failed",
            Self::AiMaskRefresh => "AI mask refresh failed",
            Self::AiUpdate => "AI update failed",
            #[cfg(not(target_os = "android"))]
            Self::AiRuntime => "AI runtime unavailable",
            Self::GpuMemory => "GPU memory exhausted",
            Self::Retouch => "Retouch failed",
            Self::Straighten => "Straightening failed",
            Self::Settings => "Settings error",
            Self::EditNotApplied => "Edit not applied",
            #[cfg(target_os = "android")]
            Self::LibraryAdjustments => "Library adjustment failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ErrorDialog {
    kind: ErrorKind,
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
    fn push(&mut self, kind: ErrorKind, message: String, now: Instant) {
        let dialog = ErrorDialog { kind, message };
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

    /// Closes every error of `kind`, for when the work that failed starts again.
    pub(crate) fn dismiss_kind(&mut self, kind: ErrorKind) {
        self.dialogs.retain(|dialog| dialog.kind != kind);
    }

    fn dismiss_current_at(&mut self, now: Instant) {
        if let Some(dialog) = self.dialogs.pop_front() {
            self.last_dismissed = Some((dialog, now));
        }
    }
}

impl CalibRawApp {
    /// Reports a failure: shows `message` in an error dialog titled by `kind`,
    /// keeps it in the status line, and logs it.
    pub(crate) fn report_error(&mut self, kind: ErrorKind, message: impl Into<String>) {
        let message = message.into();
        log::error!("{}: {message}", kind.title());
        self.ui.notice = Some(message.clone());
        self.ui.error_dialogs.push(kind, message, Instant::now());
        self.egui_ctx.request_repaint();
    }

    pub(in crate::app) fn show_error_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.ui.error_dialogs.dialogs.front().cloned() else {
            return;
        };
        let remaining = self.ui.error_dialogs.dialogs.len() - 1;
        let mut close = false;
        moduwu_design::dialog_window(
            dialog.kind.title(),
            ctx,
            moduwu_design::DIALOG_WIDTH_DEFAULT,
        )
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
        queue.push(ErrorKind::Export, "out of memory".to_owned(), now);
        queue.push(ErrorKind::Export, "out of memory".to_owned(), now);
        queue.push(ErrorKind::Export, "disk full".to_owned(), now);
        assert_eq!(queue.dialogs.len(), 2);
    }

    #[test]
    fn a_dismissed_error_stays_quiet_briefly_then_shows_again() {
        let now = Instant::now();
        let mut queue = ErrorDialogQueue::default();
        queue.push(ErrorKind::Preview, "no GPU".to_owned(), now);
        queue.dismiss_current_at(now);
        assert!(!queue.is_open());

        queue.push(
            ErrorKind::Preview,
            "no GPU".to_owned(),
            now + Duration::from_secs(1),
        );
        assert!(!queue.is_open());
        queue.push(
            ErrorKind::Export,
            "disk full".to_owned(),
            now + Duration::from_secs(1),
        );
        assert!(queue.is_open());
        queue.dismiss_current_at(now + Duration::from_secs(1));

        queue.push(
            ErrorKind::Preview,
            "no GPU".to_owned(),
            now + REPEAT_QUIET_PERIOD + Duration::from_secs(1),
        );
        assert!(queue.is_open());
    }

    #[test]
    fn dismissing_a_kind_keeps_other_errors() {
        let now = Instant::now();
        let mut queue = ErrorDialogQueue::default();
        queue.push(ErrorKind::AiMask, "no subject".to_owned(), now);
        queue.push(ErrorKind::Export, "disk full".to_owned(), now);
        queue.dismiss_kind(ErrorKind::AiMask);
        assert_eq!(queue.dialogs.len(), 1);
        assert_eq!(queue.dialogs[0].kind, ErrorKind::Export);
    }

    #[test]
    fn the_queue_is_bounded() {
        let now = Instant::now();
        let mut queue = ErrorDialogQueue::default();
        for index in 0..MAX_QUEUED_ERROR_DIALOGS + 3 {
            queue.push(ErrorKind::Export, format!("failure {index}"), now);
        }
        assert_eq!(queue.dialogs.len(), MAX_QUEUED_ERROR_DIALOGS);
        assert_eq!(queue.dialogs[0].message, "failure 0");
    }
}
