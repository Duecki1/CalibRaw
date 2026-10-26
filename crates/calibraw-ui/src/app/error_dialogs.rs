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
    Share,
    EditReplay,
    Preview,
    Sampling,
    LensCorrection,
    OpenPhoto,
    Import,
    PhotoOpenedWithProblems,
    CameraProfile,
    Presets,
    Luts,
    AiDenoise,
    AiDenoiseNotSaved,
    AiMask,
    AiMaskRefresh,
    AiUpdate,
    AiRuntime,
    AiFilesNotMoved,
    HdrMerge,
    FocusStack,
    GpuMemory,
    GpuAi,
    Retouch,
    Straighten,
    Settings,
    Tip,
    EditNotApplied,
    LibraryAdjustments,
}

impl ErrorKind {
    fn title(self) -> &'static str {
        match self {
            Self::Export => "Export failed",
            Self::ExportUnavailable => "Export unavailable",
            Self::BatchExport => "Batch export failed",
            Self::Share => "Sharing failed",
            Self::EditReplay => "Edit replay failed",
            Self::Preview => "Preview failed",
            Self::Sampling => "Sampling failed",
            Self::LensCorrection => "Lens correction failed",
            Self::OpenPhoto => "Could not open photo",
            Self::Import => "Import failed",
            Self::PhotoOpenedWithProblems => "Photo opened with problems",
            Self::CameraProfile => "Camera profile failed",
            Self::Presets => "Presets failed",
            Self::Luts => "LUTs failed",
            Self::AiDenoise => "AI denoise failed",
            Self::AiDenoiseNotSaved => "AI denoise not saved",
            Self::AiMask => "AI mask failed",
            Self::AiMaskRefresh => "AI mask refresh failed",
            Self::AiUpdate => "AI update failed",
            Self::AiRuntime => "AI runtime unavailable",
            Self::AiFilesNotMoved => "AI files not moved",
            Self::HdrMerge => "HDR merge failed",
            Self::FocusStack => "Focus stacking failed",
            Self::GpuMemory => "GPU memory exhausted",
            Self::GpuAi => "GPU AI turned off",
            Self::Retouch => "Retouch failed",
            Self::Straighten => "Straightening failed",
            Self::Settings => "Settings error",
            Self::Tip => "Tip unavailable",
            Self::EditNotApplied => "Edit not applied",
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
        moduwu_design::Dialog::new("error-dialog", dialog.kind.title()).show_with_footer(
            ctx,
            |ui| {
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
            },
            |ui| {
                close |= moduwu_design::secondary_button(ui, "Close").clicked();
                if !close
                    && moduwu_design::dialog_keyboard_action(
                        ui,
                        moduwu_design::DialogKeyboard::CLOSE_ONLY,
                        false,
                    ) == moduwu_design::DialogAction::Cancel
                {
                    close = true;
                }
            },
        );
        if close {
            self.ui.error_dialogs.dismiss_current();
        }
    }
}
