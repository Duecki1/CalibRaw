use crate::app::{CalibRawApp, ErrorKind, OnboardingStep, PreviewQuality};
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OnboardingAction {
    Back,
    Next,
    Finish,
}

pub(crate) fn show(ctx: &egui::Context, app: &mut CalibRawApp) {
    let Some(step) = app.ui.onboarding_step else {
        return;
    };
    let steps = OnboardingStep::pending(
        app.preferences.onboarding_completed,
        app.preferences.subject_mask_quality_chosen,
    );

    let mut action = None;
    moduwu_design::Dialog::new(("calibraw-first-run-onboarding", step), step.title())
        .size(moduwu_design::DialogSize::Wide)
        .show_with_footer(
            ctx,
            |ui| show_step_body(ui, app, steps, step),
            |ui| action = show_navigation(ui, steps, step),
        );

    match action {
        Some(OnboardingAction::Back) => {
            if let Some(previous) = step.previous(steps) {
                app.ui.onboarding_step = Some(previous);
            }
        }
        Some(OnboardingAction::Next) => app.ui.onboarding_step = step.next(steps),
        Some(OnboardingAction::Finish) => {
            app.preferences.onboarding_completed = true;
            app.preferences.subject_mask_quality_chosen = true;
            app.ui.onboarding_step = None;
            if !app.persist_performance_settings() {
                app.report_error(
                    ErrorKind::Settings,
                    "Setup is complete, but CalibRaw could not save the first-run preferences."
                        .to_owned(),
                );
            }
        }
        None => {}
    }
}

fn show_step_body(
    ui: &mut egui::Ui,
    app: &mut CalibRawApp,
    steps: &[OnboardingStep],
    step: OnboardingStep,
) {
    show_header(ui, steps, step);
    ui.add_space(moduwu_design::SPACE_SM);
    match step {
        OnboardingStep::Appearance => show_appearance(ui, app),
        OnboardingStep::Preview => show_preview(ui, app),
        OnboardingStep::SubjectMasks => show_subject_masks(ui, app),
        OnboardingStep::CopyPaste => show_copy_paste(ui, app),
        OnboardingStep::ExportNames => show_export_names(ui, app),
        #[cfg(not(target_os = "android"))]
        OnboardingStep::CameraProfiles => show_camera_profiles(ui, app),
        #[cfg(not(target_os = "android"))]
        OnboardingStep::Discord => show_discord(ui, app),
    }
}

/// The step's place in the setup and what it is for. The dialog's header
/// shows the step title.
fn show_header(ui: &mut egui::Ui, steps: &[OnboardingStep], step: OnboardingStep) {
    if steps.len() > 1 {
        ui.weak(format!("Step {} of {}", step.index(steps) + 1, steps.len()));
    }
    ui.label(step.introduction());
}

fn show_appearance(ui: &mut egui::Ui, app: &mut CalibRawApp) {
    let mut design = app.preferences.ui_design;
    moduwu_design::form_combo_with_help(
        ui,
        "Design",
        "onboarding-ui-design",
        design.label(),
        220.0,
        design.description(),
        |ui| {
            for option in crate::appearance::UiDesign::ALL {
                ui.selectable_value(&mut design, option, option.label())
                    .on_hover_text(option.description());
            }
        },
    );
    if design != app.preferences.ui_design {
        app.set_ui_design(design);
    }

    ui.add_space(moduwu_design::SPACE_SM);
    let mut backdrop = app.preferences.preview_backdrop;
    moduwu_design::form_combo_with_help(
        ui,
        "Preview background",
        "onboarding-preview-backdrop",
        backdrop.label(),
        220.0,
        "Sets the canvas color around the photo. Match photo derives a quiet color from each image.",
        |ui| {
            for option in crate::appearance::PreviewBackdrop::ALL {
                ui.selectable_value(&mut backdrop, option, option.label());
            }
        },
    );
    if backdrop != app.preferences.preview_backdrop {
        app.set_preview_backdrop(backdrop);
    }

    ui.add_space(moduwu_design::SPACE_SM);
    let sample_color = backdrop.color(app.ui.adaptive_preview_backdrop);
    let (sample, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(1.0), 38.0),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(sample, 6.0, sample_color);
    ui.painter().rect_stroke(
        sample,
        6.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
}

fn show_preview(ui: &mut egui::Ui, app: &mut CalibRawApp) {
    let mut quality = app.preview.quality;
    moduwu_design::form_combo_with_help(
        ui,
        "Preview quality",
        "onboarding-preview-quality",
        quality.label(),
        180.0,
        "Higher quality renders more preview pixels and uses more GPU memory. Medium is a good starting point for most displays.",
        |ui| {
            for option in [
                PreviewQuality::Low,
                PreviewQuality::Medium,
                PreviewQuality::High,
                PreviewQuality::Max,
            ] {
                ui.selectable_value(&mut quality, option, option.label());
            }
        },
    );
    if quality != app.preview.quality {
        app.preview.quality = quality;
        app.preview_quality_changed();
    }

    ui.add_space(moduwu_design::SPACE_SM);
    ui.small(match quality {
        PreviewQuality::Low => "75% render density · lowest GPU memory use",
        PreviewQuality::Medium => "100% render density · recommended balance",
        PreviewQuality::High => "125% render density · finer previews",
        PreviewQuality::Max => "150% render density · highest GPU memory use",
    });
}

fn show_subject_masks(ui: &mut egui::Ui, app: &mut CalibRawApp) {
    crate::ui::settings::subject_mask_quality_control(ui, app, "onboarding-subject-mask-quality");
    ui.add_space(moduwu_design::SPACE_SM);
    ui.small(crate::ui::settings::subject_mask_quality_descriptions());
}

fn show_copy_paste(ui: &mut egui::Ui, app: &mut CalibRawApp) {
    let mut settings = app.preferences.adjustment_copy_settings;
    let mut changed = false;
    changed |= moduwu_design::toggle_with_help(
        ui,
        &mut settings.adjustments,
        "Adjustments",
        "Exposure, color, tone, detail, effects, and other Develop adjustments.",
    )
    .changed();
    changed |= moduwu_design::toggle_with_help(
        ui,
        &mut settings.geometry,
        "Crop & geometry",
        "Crop, rotation, transforms, and flips.",
    )
    .changed();
    changed |= moduwu_design::toggle_with_help(
        ui,
        &mut settings.camera_profile,
        "Camera profile",
        "The selected camera profile, when it is available for the destination image.",
    )
    .changed();
    changed |= moduwu_design::toggle_with_help(
        ui,
        &mut settings.masks,
        "Manual masks",
        "Brush, linear, radial, and fullscreen mask components with their local edits.",
    )
    .changed();
    changed |= moduwu_design::toggle_with_help(
        ui,
        &mut settings.ai_masks,
        "Content-aware masks",
        "Subject, background, sky, object, luminance-range, and color-range components. They are regenerated for the destination image when needed.",
    )
    .changed();
    changed |= moduwu_design::toggle_with_help(
        ui,
        &mut settings.lens_correction,
        "Lens correction",
        "The enabled state and selected lens profile.",
    )
    .changed();
    if changed {
        app.set_adjustment_copy_settings(settings);
    }
}

fn show_export_names(ui: &mut egui::Ui, app: &mut CalibRawApp) {
    crate::ui::settings::export_name_template_controls(ui, app, "onboarding-export-name-template");
}

#[cfg(not(target_os = "android"))]
fn show_camera_profiles(ui: &mut egui::Ui, app: &mut CalibRawApp) {
    let auto_detect_available =
        !crate::performance_settings::CAMERA_PROFILE_SEARCH_LOCATIONS.is_empty();
    if auto_detect_available {
        ui.small("Auto-detect looks for a CameraProfiles folder in:");
        for location in crate::performance_settings::CAMERA_PROFILE_SEARCH_LOCATIONS {
            ui.add(egui::Label::new(egui::RichText::new(*location).monospace().small()).wrap());
        }
        ui.add_space(moduwu_design::SPACE_SM);
    }

    moduwu_design::action_row(ui, |ui| {
        if auto_detect_available
            && moduwu_design::primary_action_button(ui, "Auto-detect (Recommended)").clicked()
        {
            app.auto_detect_camera_profile_folder();
        }
        if moduwu_design::secondary_button_enabled(
            ui,
            app.ui.desktop_picker_receiver.is_none(),
            "Choose folder…",
        )
        .on_hover_text("CalibRaw searches subfolders recursively.")
        .clicked()
        {
            app.choose_camera_profile_folder();
        }
        if moduwu_design::secondary_button(ui, "Not now").clicked() {
            app.clear_camera_profile_folder();
        }
    });

    ui.add_space(moduwu_design::SPACE_SM);
    if let Some(folder) = &app.preferences.camera_profile_folder {
        ui.small(
            app.preferences
                .camera_profile_folder_label
                .as_deref()
                .unwrap_or("DCP profiles"),
        );
        ui.add(
            egui::Label::new(egui::RichText::new(folder.display().to_string()).monospace()).wrap(),
        );
    } else if auto_detect_available && app.preferences.camera_profile_auto_detect {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "No CameraProfiles folder was found.",
        );
    } else {
        ui.small("No DCP profile folder selected. You can add one later in Settings.");
    }
}

#[cfg(not(target_os = "android"))]
fn show_discord(ui: &mut egui::Ui, app: &mut CalibRawApp) {
    let mut enabled = app.preferences.discord_rich_presence;
    if moduwu_design::toggle_with_help(
        ui,
        &mut enabled,
        "Enable Discord Rich Presence",
        "Shares only whether you are browsing the Library or editing, plus the elapsed edit time. Photo names and paths are never sent. Discord's desktop client must be running.",
    )
    .changed()
    {
        app.set_discord_rich_presence(enabled);
    }

    ui.add_space(moduwu_design::SPACE_SM);
    ui.small("This is optional and disabled by default. You can change it later in Settings.");
    if !app.discord_rich_presence_configured() {
        ui.add_space(moduwu_design::SPACE_SM);
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Unavailable in this build: CALIBRAW_DISCORD_APPLICATION_ID is not configured.",
        );
    }
}

fn show_navigation(
    ui: &mut egui::Ui,
    steps: &[OnboardingStep],
    step: OnboardingStep,
) -> Option<OnboardingAction> {
    // Footer actions are added primary first.
    let (label, next) = if step.next(steps).is_some() {
        ("Next", OnboardingAction::Next)
    } else if steps.len() > 1 {
        ("Finish setup", OnboardingAction::Finish)
    } else {
        ("Done", OnboardingAction::Finish)
    };
    let forward = moduwu_design::primary_action_button(ui, label).clicked();
    let back = moduwu_design::secondary_button_enabled(ui, step.previous(steps).is_some(), "Back")
        .clicked();
    if forward {
        Some(next)
    } else {
        back.then_some(OnboardingAction::Back)
    }
}

impl OnboardingStep {
    /// Setup order; navigation and step numbers derive from it. Discord stays
    /// last so builds without Rich Presence can drop it from the end.
    const ALL: &'static [Self] = &[
        Self::Appearance,
        Self::Preview,
        Self::SubjectMasks,
        Self::CopyPaste,
        Self::ExportNames,
        #[cfg(not(target_os = "android"))]
        Self::CameraProfiles,
        #[cfg(not(target_os = "android"))]
        Self::Discord,
    ];

    /// `ALL` for builds without Discord Rich Presence.
    #[cfg(not(target_os = "android"))]
    const ALL_WITHOUT_DISCORD: &'static [Self] = match Self::ALL.split_last() {
        Some((Self::Discord, rest)) => rest,
        _ => panic!("the Discord step must be the last setup step"),
    };
    #[cfg(target_os = "android")]
    const ALL_WITHOUT_DISCORD: &'static [Self] = Self::ALL;

    /// Steps added after setup first shipped, shown alone to users who
    /// finished an earlier setup.
    const CATCH_UP: &'static [Self] = &[Self::SubjectMasks];

    /// The steps a user still has to go through, empty when none.
    pub(crate) fn pending(
        onboarding_completed: bool,
        subject_mask_quality_chosen: bool,
    ) -> &'static [Self] {
        #[cfg(not(target_os = "android"))]
        let discord_supported = crate::desktop_portal::discord_presence_supported();
        #[cfg(target_os = "android")]
        let discord_supported = false;
        Self::pending_with(
            onboarding_completed,
            subject_mask_quality_chosen,
            discord_supported,
        )
    }

    fn pending_with(
        onboarding_completed: bool,
        subject_mask_quality_chosen: bool,
        discord_supported: bool,
    ) -> &'static [Self] {
        match (onboarding_completed, subject_mask_quality_chosen) {
            (false, _) if discord_supported => Self::ALL,
            (false, _) => Self::ALL_WITHOUT_DISCORD,
            (true, false) => Self::CATCH_UP,
            (true, true) => &[],
        }
    }

    fn index(self, steps: &[Self]) -> usize {
        // The shown step is always one of the pending `steps`.
        steps
            .iter()
            .position(|&step| step == self)
            .unwrap_or_default()
    }

    fn next(self, steps: &[Self]) -> Option<Self> {
        steps.get(self.index(steps) + 1).copied()
    }

    fn previous(self, steps: &[Self]) -> Option<Self> {
        self.index(steps)
            .checked_sub(1)
            .and_then(|index| steps.get(index).copied())
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Appearance => "Welcome to CalibRaw",
            Self::Preview => "Preview setup",
            Self::SubjectMasks => "Subject masks",
            Self::CopyPaste => "Copy & paste setup",
            Self::ExportNames => "Export file names",
            #[cfg(not(target_os = "android"))]
            Self::CameraProfiles => "Camera profiles",
            #[cfg(not(target_os = "android"))]
            Self::Discord => "Discord Rich Presence",
        }
    }

    const fn introduction(self) -> &'static str {
        match self {
            Self::Appearance => {
                "Choose how CalibRaw looks and the background shown around your photographs."
            }
            Self::Preview => {
                "Choose the balance between interactive preview detail and GPU memory use."
            }
            Self::SubjectMasks => {
                "Choose the balance between AI subject selection speed and edge detail on this device. Models are downloaded when you first select a subject."
            }
            Self::CopyPaste => {
                "Choose which edit categories are included when copying and pasting adjustments."
            }
            Self::ExportNames => {
                "Choose how CalibRaw names exported files. You can combine source details with your own text."
            }
            #[cfg(not(target_os = "android"))]
            Self::CameraProfiles => {
                "Choose where CalibRaw finds DCP camera profiles for your RAW files."
            }
            #[cfg(not(target_os = "android"))]
            Self::Discord => {
                "Choose whether Discord may show that you are browsing or editing in CalibRaw."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_unique() {
        for steps in [OnboardingStep::ALL, OnboardingStep::CATCH_UP] {
            for (index, step) in steps.iter().enumerate() {
                assert_eq!(step.index(steps), index, "{step:?} appears more than once");
            }
        }
    }

    #[test]
    fn next_and_previous_walk_the_step_order() {
        let steps = OnboardingStep::ALL;
        assert_eq!(steps.first(), Some(&OnboardingStep::Appearance));
        assert_eq!(steps[0].previous(steps), None);
        assert_eq!(steps[steps.len() - 1].next(steps), None);
        for pair in steps.windows(2) {
            assert_eq!(pair[0].next(steps), Some(pair[1]));
            assert_eq!(pair[1].previous(steps), Some(pair[0]));
        }
    }

    #[test]
    fn finished_setup_shows_only_steps_added_since() {
        let pending = |completed, chosen| OnboardingStep::pending_with(completed, chosen, true);
        assert_eq!(pending(false, false), OnboardingStep::ALL);
        assert_eq!(pending(false, true), OnboardingStep::ALL);
        assert_eq!(pending(true, false), [OnboardingStep::SubjectMasks]);
        assert!(pending(true, true).is_empty());
        for &step in OnboardingStep::CATCH_UP {
            assert!(OnboardingStep::ALL.contains(&step));
        }
        // A lone catch-up step offers no way back.
        let steps = pending(true, false);
        assert_eq!(steps[0].previous(steps), None);
        assert_eq!(steps[0].next(steps), None);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn builds_without_discord_skip_only_its_step() {
        let steps = OnboardingStep::pending_with(false, false, false);
        assert!(!steps.contains(&OnboardingStep::Discord));
        assert_eq!(steps.len(), OnboardingStep::ALL.len() - 1);
        assert_eq!(steps.last(), Some(&OnboardingStep::CameraProfiles));
        assert_eq!(steps[steps.len() - 1].next(steps), None);
        assert_eq!(
            OnboardingStep::pending_with(true, false, false),
            [OnboardingStep::SubjectMasks]
        );
    }
}
