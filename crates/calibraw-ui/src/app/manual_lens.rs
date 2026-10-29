//! Manual lens correction: the adjust dialog with its live preview, and the
//! prompt that offers manual correction when automatic correction finds no
//! Lensfun profile for a photo that just opened.

use super::*;
use crate::i18n::{tr, tr_fmt};

impl CalibRawApp {
    /// Asks about the photo that just opened without its automatic lens
    /// correction, unless the user turned the prompt off.
    pub(in crate::app) fn prompt_for_missing_lens_profile(&mut self, raw: &LoadedRaw) {
        if !self.preferences.missing_lens_profile_prompt {
            return;
        }
        let lens = match (raw.lens_make.trim(), raw.lens_model.trim()) {
            (_, "") => None,
            ("", model) => Some(model.to_owned()),
            (maker, model) if model.starts_with(maker) => Some(model.to_owned()),
            (maker, model) => Some(format!("{maker} {model}")),
        };
        self.ui.missing_lens_profile_prompt = Some(match lens {
            Some(lens) => tr_fmt(
                "No lens profile matches {lens}, so automatic lens correction was not applied.",
                &[("lens", &lens)],
            ),
            None => tr(
                "The photo does not name its lens, so automatic lens correction was not applied.",
            )
            .to_owned(),
        });
    }

    pub(in crate::app) fn show_missing_lens_profile_dialog(&mut self, ctx: &egui::Context) {
        let Some(message) = self.ui.missing_lens_profile_prompt.clone() else {
            return;
        };
        let mut close = false;
        let mut manual = false;
        let mut never_again = false;
        moduwu_design::Dialog::new("missing-lens-profile", tr("Lens correction not applied"))
            .show_with_footer(
                ctx,
                |ui| {
                    ui.label(&message);
                },
                |ui| {
                    close |= moduwu_design::primary_action_button(ui, tr("OK")).clicked();
                    manual |= moduwu_design::secondary_button(ui, tr("Correct manually")).clicked();
                    never_again |=
                        moduwu_design::secondary_button(ui, tr("Never show again")).clicked();
                    if !(close || manual || never_again)
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
        if !(close || manual || never_again) {
            return;
        }
        self.ui.missing_lens_profile_prompt = None;
        if never_again {
            self.set_missing_lens_profile_prompt(false);
        }
        if manual {
            self.start_manual_lens_correction();
        }
    }

    /// Opens the adjust dialog on the current manual values.
    pub(crate) fn open_manual_lens_dialog(&mut self) {
        let lens = &self.develop.lens_correction;
        let current = lens.manual.unwrap_or_default();
        // In manual mode an applied correction is the current values, and its
        // vignetting is already in the preview pixels.
        let shown = if lens.applied {
            current
        } else {
            ManualLensCorrection::default()
        };
        self.develop_ui.manual_lens_dialog = Some(ManualLensDialog {
            draft: current,
            shown,
        });
    }

    /// Closes the adjust dialog and corrects the photo with `correction`.
    pub(crate) fn apply_manual_lens_correction(&mut self, correction: ManualLensCorrection) {
        self.develop_ui.manual_lens_dialog = None;
        let lens = &mut self.develop.lens_correction;
        if lens.enabled && lens.manual == Some(correction) {
            return;
        }
        lens.manual = Some(correction);
        lens.enabled = true;
        self.lens_correction_edited();
    }

    /// Shows the Lens Corrections card of the Edit tab in manual mode.
    pub(crate) fn start_manual_lens_correction(&mut self) {
        self.activate_tab(AppTab::Develop);
        if self.ui.sidebar_tab == SidebarTab::Adjustments {
            self.develop_ui.sidebar_open = true;
        } else {
            self.dispatch_action(AppAction::SelectSidebarTab(SidebarTab::Adjustments));
        }
        self.develop_ui.adjustment_section = AdjustmentSection::Optics;
        self.develop_ui.reveal_lens_card = true;
        if self.develop.lens_correction.manual.is_none() {
            self.develop.lens_correction.set_manual_mode(true);
            self.lens_correction_edited();
        }
    }
}
