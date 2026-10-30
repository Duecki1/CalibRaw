use crate::app::{CalibRawApp, ErrorKind, OnboardingStep, PreviewQuality};
use eframe::egui;

use crate::i18n::{tr, tr_fmt};

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
    let steps = OnboardingStep::pending(app);

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
            app.preferences.people_mask_quality_chosen = true;
            app.ui.onboarding_step = None;
            if !app.persist_performance_settings() {
                app.report_error(
                    ErrorKind::Settings,
                    tr("Setup is complete, but CalibRaw could not save the first-run preferences.")
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
        OnboardingStep::CameraProfiles => show_camera_profiles(ui, app),
        OnboardingStep::Discord => show_discord(ui, app),
    }
}

/// The step's place in the setup and what it is for. The dialog's header
/// shows the step title.
fn show_header(ui: &mut egui::Ui, steps: &[OnboardingStep], step: OnboardingStep) {
    if steps.len() > 1 {
        ui.weak(tr_fmt(
            "Step {step} of {count}",
            &[("step", &(step.index(steps) + 1)), ("count", &steps.len())],
        ));
    }
    ui.label(step.introduction());
}

fn show_appearance(ui: &mut egui::Ui, app: &mut CalibRawApp) {
    // Asked first so the remaining setup steps already speak the chosen language.
    crate::ui::settings::language_control(ui, app, "onboarding-language");

    ui.add_space(moduwu_design::SPACE_SM);
    let mut design = app.preferences.ui_design;
    moduwu_design::form_combo_with_help(
        ui,
        tr("Design"),
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
        tr("Preview background"),
        "onboarding-preview-backdrop",
        backdrop.label(),
        220.0,
        tr("Sets the canvas color around the photo. Match photo derives a quiet color from each image."),
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
    let constrained = app.caps().constrained_memory();
    let mut quality = app.preview.quality;
    let help = if constrained {
        tr("Higher quality renders more preview pixels and uses more GPU memory. Max matches your screen pixel for pixel.")
    } else {
        tr("Higher quality renders more preview pixels and uses more GPU memory. Max renders 150% of your screen pixels and averages them down, for the sharpest preview.")
    };
    moduwu_design::form_combo_with_help(
        ui,
        tr("Preview quality"),
        "onboarding-preview-quality",
        quality.label(),
        180.0,
        help,
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
    ui.small(if constrained {
        match quality {
            PreviewQuality::Low => tr("50% render density · lowest GPU memory use"),
            PreviewQuality::Medium => tr("75% render density · lighter on the GPU"),
            PreviewQuality::High => tr("90% render density · nearly exact previews"),
            PreviewQuality::Max => tr("100% render density · exact screen pixels"),
        }
    } else {
        match quality {
            PreviewQuality::Low => tr("75% render density · lowest GPU memory use"),
            PreviewQuality::Medium => tr("100% render density · exact screen pixels"),
            PreviewQuality::High => tr("125% render density · finer detail"),
            PreviewQuality::Max => tr("150% render density · supersampled previews"),
        }
    });
}

fn show_subject_masks(ui: &mut egui::Ui, app: &mut CalibRawApp) {
    crate::ui::settings::subject_mask_quality_control(ui, app, "onboarding-subject-mask-quality");
    crate::ui::settings::people_mask_quality_control(ui, app, "onboarding-people-mask-quality");
    ui.add_space(moduwu_design::SPACE_SM);
    ui.small(crate::ui::settings::subject_mask_quality_descriptions());
}

fn show_copy_paste(ui: &mut egui::Ui, app: &mut CalibRawApp) {
    let mut settings = app.preferences.adjustment_copy_settings;
    let mut changed = false;
    changed |= moduwu_design::toggle_with_help(
        ui,
        &mut settings.adjustments,
        tr("Adjustments"),
        tr("Exposure, color, tone, detail, effects, and other Develop adjustments."),
    )
    .changed();
    changed |= moduwu_design::toggle_with_help(
        ui,
        &mut settings.geometry,
        tr("Crop & geometry"),
        tr("Crop, rotation, transforms, and flips."),
    )
    .changed();
    changed |= moduwu_design::toggle_with_help(
        ui,
        &mut settings.camera_profile,
        tr("Camera profile"),
        tr("The selected camera profile, when it is available for the destination image."),
    )
    .changed();
    changed |= moduwu_design::toggle_with_help(
        ui,
        &mut settings.lut,
        tr("LUT"),
        tr("The LUT and its amount."),
    )
    .changed();
    changed |= moduwu_design::toggle_with_help(
        ui,
        &mut settings.masks,
        tr("Manual masks"),
        tr("Brush, linear, radial, and fullscreen mask components with their local edits."),
    )
    .changed();
    changed |= moduwu_design::toggle_with_help(
        ui,
        &mut settings.ai_masks,
        tr("Content-aware masks"),
        tr("Subject, background, sky, object, luminance-range, and color-range components. They are regenerated for the destination image when needed."),
    )
    .changed();
    changed |= moduwu_design::toggle_with_help(
        ui,
        &mut settings.lens_correction,
        tr("Lens correction"),
        tr("The enabled state and selected lens profile."),
    )
    .changed();
    if changed {
        app.set_adjustment_copy_settings(settings);
    }
}

fn show_export_names(ui: &mut egui::Ui, app: &mut CalibRawApp) {
    crate::ui::settings::export_name_template_controls(ui, app, "onboarding-export-name-template");
}

fn show_camera_profiles(ui: &mut egui::Ui, app: &mut CalibRawApp) {
    let auto_detect_available = !crate::platform::camera_profile_search_locations().is_empty();
    if auto_detect_available {
        ui.small(tr("Auto-detect looks for a CameraProfiles folder in:"));
        for location in crate::platform::camera_profile_search_locations() {
            ui.add(egui::Label::new(egui::RichText::new(*location).monospace().small()).wrap());
        }
        ui.add_space(moduwu_design::SPACE_SM);
    }

    moduwu_design::action_row(ui, |ui| {
        if auto_detect_available
            && moduwu_design::primary_action_button(ui, tr("Auto-detect (Recommended)")).clicked()
        {
            app.auto_detect_camera_profile_folder();
        }
        if moduwu_design::secondary_button_enabled(
            ui,
            app.ui.desktop_picker_receiver.is_none(),
            tr("Choose folder…"),
        )
        .on_hover_text(tr("CalibRaw searches subfolders recursively."))
        .clicked()
        {
            app.choose_camera_profile_folder();
        }
        if moduwu_design::secondary_button(ui, tr("Not now")).clicked() {
            app.clear_camera_profile_folder();
        }
    });

    ui.add_space(moduwu_design::SPACE_SM);
    if let Some(folder) = &app.preferences.camera_profile_folder {
        ui.small(
            app.preferences
                .camera_profile_folder_label
                .as_deref()
                .unwrap_or(tr("DCP profiles")),
        );
        ui.add(
            egui::Label::new(egui::RichText::new(folder.display().to_string()).monospace()).wrap(),
        );
    } else if auto_detect_available && app.preferences.camera_profile_auto_detect {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            tr("No CameraProfiles folder was found."),
        );
    } else {
        ui.small(tr(
            "No DCP profile folder selected. You can add one later in Settings.",
        ));
    }
}

fn show_discord(ui: &mut egui::Ui, app: &mut CalibRawApp) {
    let mut enabled = app.preferences.discord_rich_presence;
    if moduwu_design::toggle_with_help(
        ui,
        &mut enabled,
        tr("Enable Discord Rich Presence"),
        tr("Shares only whether you are browsing the Library or editing, plus the elapsed edit time. Photo names and paths are never sent. Discord's desktop client must be running."),
    )
    .changed()
    {
        app.set_discord_rich_presence(enabled);
    }

    ui.add_space(moduwu_design::SPACE_SM);
    ui.small(tr(
        "This is optional and disabled by default. You can change it later in Settings.",
    ));
    if !app.discord_rich_presence_configured() {
        ui.add_space(moduwu_design::SPACE_SM);
        ui.colored_label(
            ui.visuals().warn_fg_color,
            tr("Unavailable in this build: CALIBRAW_DISCORD_APPLICATION_ID is not configured."),
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
        (tr("Next"), OnboardingAction::Next)
    } else if steps.len() > 1 {
        (tr("Finish setup"), OnboardingAction::Finish)
    } else {
        (tr("Done"), OnboardingAction::Finish)
    };
    let forward = moduwu_design::primary_action_button(ui, label).clicked();
    let back =
        moduwu_design::secondary_button_enabled(ui, step.previous(steps).is_some(), tr("Back"))
            .clicked();
    if forward {
        Some(next)
    } else {
        back.then_some(OnboardingAction::Back)
    }
}

impl OnboardingStep {
    /// Setup order; navigation and step numbers derive from it. Camera
    /// profiles and Discord stay last so platforms without a camera profile
    /// folder or Rich Presence can drop them from the end.
    const ALL: &'static [Self] = &[
        Self::Appearance,
        Self::Preview,
        Self::SubjectMasks,
        Self::CopyPaste,
        Self::ExportNames,
        Self::CameraProfiles,
        Self::Discord,
    ];

    /// `ALL` for builds without Discord Rich Presence.
    const ALL_WITHOUT_DISCORD: &'static [Self] = match Self::ALL.split_last() {
        Some((Self::Discord, rest)) => rest,
        _ => panic!("the Discord step must be the last setup step"),
    };

    /// `ALL` where camera profiles are imported through the system picker in
    /// Settings, which has no Rich Presence either.
    const ALL_WITHOUT_CAMERA_PROFILES: &'static [Self] =
        match Self::ALL_WITHOUT_DISCORD.split_last() {
            Some((Self::CameraProfiles, rest)) => rest,
            _ => panic!("the camera profile step must precede the Discord step"),
        };

    /// Steps added after setup first shipped, shown alone to users who
    /// finished an earlier setup.
    const CATCH_UP: &'static [Self] = &[Self::SubjectMasks];

    /// The steps a user still has to go through, empty when none.
    pub(crate) fn pending(app: &CalibRawApp) -> &'static [Self] {
        use calibraw_ai::models::people::PeopleMaskQuality;
        // The subject step asks for every mask quality this device offers,
        // including the people-mask quality it gained: a user who finished an
        // earlier setup that had none still has that choice outstanding and is
        // shown the step once again. Where the device offers a single people
        // quality there is nothing left to ask.
        let mask_choices_outstanding =
            !app.preferences.subject_mask_quality_chosen
                || !(app.preferences.people_mask_quality_chosen
                    || PeopleMaskQuality::available().len() < 2);
        Self::pending_with(
            app.preferences.onboarding_completed,
            mask_choices_outstanding,
            app.caps().has_folders(),
            crate::platform::discord::presence_supported(),
        )
    }

    /// `mask_choices_outstanding`: setup has a mask-quality choice this device
    /// offers that the user has not made.
    fn pending_with(
        onboarding_completed: bool,
        mask_choices_outstanding: bool,
        camera_profile_folders: bool,
        discord_supported: bool,
    ) -> &'static [Self] {
        match (onboarding_completed, mask_choices_outstanding) {
            (false, _) if !camera_profile_folders => Self::ALL_WITHOUT_CAMERA_PROFILES,
            (false, _) if discord_supported => Self::ALL,
            (false, _) => Self::ALL_WITHOUT_DISCORD,
            (true, true) => Self::CATCH_UP,
            (true, false) => &[],
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

    fn title(self) -> &'static str {
        match self {
            Self::Appearance => tr("Welcome to CalibRaw"),
            Self::Preview => tr("Preview setup"),
            Self::SubjectMasks => tr("Subject masks"),
            Self::CopyPaste => tr("Copy & paste setup"),
            Self::ExportNames => tr("Export file names"),
            Self::CameraProfiles => tr("Camera profiles"),
            Self::Discord => tr("Discord Rich Presence"),
        }
    }

    fn introduction(self) -> &'static str {
        match self {
            Self::Appearance => {
                tr("Choose how CalibRaw looks and the background shown around your photographs.")
            }
            Self::Preview => {
                tr("Choose the balance between interactive preview detail and GPU memory use.")
            }
            Self::SubjectMasks => {
                tr("Choose the balance between AI subject selection speed and edge detail on this device. Models are downloaded when you first select a subject.")
            }
            Self::CopyPaste => {
                tr("Choose which edit categories are included when copying and pasting adjustments.")
            }
            Self::ExportNames => {
                tr("Choose how CalibRaw names exported files. You can combine source details with your own text.")
            }
            Self::CameraProfiles => {
                tr("Choose where CalibRaw finds DCP camera profiles for your RAW files.")
            }
            Self::Discord => {
                tr("Choose whether Discord may show that you are browsing or editing in CalibRaw.")
            }
        }
    }
}
