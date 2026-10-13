//! Manual lens correction: the adjust dialog with its live preview, and the
//! prompt that offers manual correction when automatic correction finds no
//! Lensfun profile for a photo that just opened.

use super::*;

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
            Some(lens) => format!(
                "No lens profile matches {lens}, so automatic lens correction was not applied."
            ),
            None => "The photo does not name its lens, so automatic lens correction was not \
                     applied."
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
        moduwu_design::dialog_window(
            "Lens correction not applied",
            ctx,
            moduwu_design::DIALOG_WIDTH_DEFAULT,
        )
        .show(ctx, |ui| {
            ui.label(&message);
            moduwu_design::dialog_button_row(ui, |ui| {
                close |= moduwu_design::primary_action_button(ui, "OK").clicked();
                manual |= moduwu_design::secondary_button(ui, "Correct manually").clicked();
                never_again |= moduwu_design::secondary_button(ui, "Never show again").clicked();
            });
            if !(close || manual || never_again)
                && moduwu_design::dialog_keyboard_action(
                    ui,
                    moduwu_design::DialogKeyboard::CLOSE_ONLY,
                    false,
                ) == moduwu_design::DialogAction::Cancel
            {
                close = true;
            }
        });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_correction_opens_the_lens_card_without_correcting_yet() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.ui.sidebar_tab = SidebarTab::Masks;
        app.develop_ui.sidebar_open = false;

        app.start_manual_lens_correction();

        assert_eq!(app.ui.active_tab, AppTab::Develop);
        assert_eq!(app.ui.sidebar_tab, SidebarTab::Adjustments);
        assert!(app.develop_ui.sidebar_open);
        assert_eq!(app.develop_ui.adjustment_section, AdjustmentSection::Optics);
        assert!(app.develop_ui.reveal_lens_card);
        let lens = &app.develop.lens_correction;
        assert!(lens.enabled);
        assert_eq!(lens.manual, Some(ManualLensCorrection::default()));
        // Neutral manual values change nothing the photo shows.
        assert!(!app.lens_correction_busy());
    }

    #[test]
    fn the_dialog_previews_from_what_is_applied_and_apply_corrects() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        let applied = ManualLensCorrection {
            distortion: 10.0,
            vignetting: 20.0,
        };
        let lens = &mut app.develop.lens_correction;
        lens.enabled = true;
        lens.applied = true;
        lens.manual = Some(applied);
        app.open_manual_lens_dialog();
        let dialog = app.develop_ui.manual_lens_dialog.unwrap();
        assert_eq!((dialog.draft, dialog.shown), (applied, applied));

        // Applying the values already shown changes nothing.
        app.apply_manual_lens_correction(applied);
        assert!(app.develop_ui.manual_lens_dialog.is_none());
        assert!(!app.lens_correction_busy());

        app.develop.lens_correction.applied = false;
        app.develop.lens_correction.enabled = false;
        app.open_manual_lens_dialog();
        let dialog = app.develop_ui.manual_lens_dialog.unwrap();
        assert_eq!(dialog.shown, ManualLensCorrection::default());
        let adjusted = ManualLensCorrection {
            distortion: -15.0,
            ..applied
        };
        app.apply_manual_lens_correction(adjusted);
        let lens = &app.develop.lens_correction;
        assert!(lens.enabled);
        assert_eq!(lens.manual, Some(adjusted));
    }

    #[test]
    fn the_prompt_names_the_lens_and_can_be_turned_off() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        let mut raw = LoadedRaw::from_scene_linear_rec2020(1, 1, vec![0.0; 3]).unwrap();
        raw.lens_make = "Samyang".to_owned();
        raw.lens_model = "AF 35mm F1.4".to_owned();
        app.prompt_for_missing_lens_profile(&raw);
        assert!(app
            .ui
            .missing_lens_profile_prompt
            .as_deref()
            .is_some_and(|message| message.contains("Samyang AF 35mm F1.4")));

        app.ui.missing_lens_profile_prompt = None;
        app.preferences.missing_lens_profile_prompt = false;
        app.prompt_for_missing_lens_profile(&raw);
        assert!(app.ui.missing_lens_profile_prompt.is_none());
    }
}
