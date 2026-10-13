use crate::app::{CalibRawApp, ErrorKind, OnboardingStep, PreviewQuality};
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OnboardingAction {
    Back,
    Next,
    Finish,
}

#[derive(Clone, Copy, Debug)]
struct OnboardingLayoutMeasurement {
    step: OnboardingStep,
    width_bits: u32,
    height_bits: u32,
    needs_scroll: bool,
}

const ONBOARDING_LAYOUT_MEASUREMENT_ID: &str = "calibraw-first-run-onboarding-layout";
const ONBOARDING_FOOTER_EXTRA_HEIGHT: f32 = 30.0;

pub(crate) fn show(ctx: &egui::Context, app: &mut CalibRawApp) {
    let Some(step) = app.ui.onboarding_step else {
        return;
    };

    let available = ctx.content_rect().size() - egui::vec2(32.0, 32.0);
    let width = available.x.clamp(1.0, 540.0);
    let max_body_height =
        (available.y.max(1.0) - moduwu_design::CONTROL_HEIGHT - ONBOARDING_FOOTER_EXTRA_HEIGHT)
            .max(1.0);

    let action = egui::Modal::new(egui::Id::new(("calibraw-first-run-onboarding", step)))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.set_max_width(width);
            show_fitted_step_body(ui, app, step, width, max_body_height);
            ui.add_space(moduwu_design::SPACE_MD);
            ui.separator();
            ui.add_space(moduwu_design::SPACE_SM);
            show_navigation(ui, step)
        })
        .inner;

    match action {
        Some(OnboardingAction::Back) => {
            if let Some(previous) = step.previous() {
                app.ui.onboarding_step = Some(previous);
            }
        }
        Some(OnboardingAction::Next) => app.ui.onboarding_step = step.next(),
        Some(OnboardingAction::Finish) => {
            app.preferences.onboarding_completed = true;
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

/// Shows the step body, wrapped in a scroll area only when it does not fit.
/// The first frame for a step and size measures inside a scroll area, caches
/// the result and discards the frame if the scroll area was unnecessary.
fn show_fitted_step_body(
    ui: &mut egui::Ui,
    app: &mut CalibRawApp,
    step: OnboardingStep,
    width: f32,
    max_body_height: f32,
) {
    let id = egui::Id::new(ONBOARDING_LAYOUT_MEASUREMENT_ID);
    let ctx = ui.ctx().clone();
    let needs_scroll = ctx
        .data(|data| data.get_temp::<OnboardingLayoutMeasurement>(id))
        .filter(|measurement| {
            measurement.step == step
                && measurement.width_bits == width.to_bits()
                && measurement.height_bits == max_body_height.to_bits()
        })
        .map(|measurement| measurement.needs_scroll);

    if needs_scroll == Some(false) {
        show_step_body(ui, app, step);
        return;
    }
    let output = egui::ScrollArea::vertical()
        .max_height(max_body_height)
        .auto_shrink([false, true])
        .show(ui, |ui| show_step_body(ui, app, step));
    if needs_scroll.is_some() {
        return;
    }
    let needs_scroll = output.content_size.y > output.inner_rect.height() + 1.0;
    ctx.data_mut(|data| {
        data.insert_temp(
            id,
            OnboardingLayoutMeasurement {
                step,
                width_bits: width.to_bits(),
                height_bits: max_body_height.to_bits(),
                needs_scroll,
            },
        );
    });
    if !needs_scroll {
        ctx.request_discard("onboarding content fits without scrolling");
    }
}

fn show_step_body(ui: &mut egui::Ui, app: &mut CalibRawApp, step: OnboardingStep) {
    show_header(ui, step);
    ui.add_space(moduwu_design::SPACE_SM);
    match step {
        OnboardingStep::Appearance => show_appearance(ui, app),
        OnboardingStep::Preview => show_preview(ui, app),
        OnboardingStep::CopyPaste => show_copy_paste(ui, app),
        OnboardingStep::ExportNames => show_export_names(ui, app),
        #[cfg(not(target_os = "android"))]
        OnboardingStep::CameraProfiles => show_camera_profiles(ui, app),
        #[cfg(not(target_os = "android"))]
        OnboardingStep::Discord => show_discord(ui, app),
    }
}

fn show_header(ui: &mut egui::Ui, step: OnboardingStep) {
    ui.horizontal(|ui| {
        ui.heading(step.title());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.weak(format!(
                "Step {} of {}",
                step.index() + 1,
                OnboardingStep::ALL.len()
            ));
        });
    });
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

fn show_navigation(ui: &mut egui::Ui, step: OnboardingStep) -> Option<OnboardingAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        if moduwu_design::secondary_button_enabled(ui, step.previous().is_some(), "Back").clicked()
        {
            action = Some(OnboardingAction::Back);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (label, next) = if step.next().is_some() {
                ("Next", OnboardingAction::Next)
            } else {
                ("Finish setup", OnboardingAction::Finish)
            };
            if moduwu_design::secondary_button(ui, label).clicked() {
                action = Some(next);
            }
        });
    });
    action
}

impl OnboardingStep {
    /// Setup order; navigation and step numbers derive from it.
    const ALL: &'static [Self] = &[
        Self::Appearance,
        Self::Preview,
        Self::CopyPaste,
        Self::ExportNames,
        #[cfg(not(target_os = "android"))]
        Self::CameraProfiles,
        #[cfg(not(target_os = "android"))]
        Self::Discord,
    ];

    fn index(self) -> usize {
        // Every variant compiled for this platform is listed in `ALL`.
        Self::ALL
            .iter()
            .position(|&step| step == self)
            .unwrap_or_default()
    }

    fn next(self) -> Option<Self> {
        Self::ALL.get(self.index() + 1).copied()
    }

    fn previous(self) -> Option<Self> {
        self.index()
            .checked_sub(1)
            .and_then(|index| Self::ALL.get(index).copied())
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Appearance => "Welcome to CalibRaw",
            Self::Preview => "Preview setup",
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
        for (index, step) in OnboardingStep::ALL.iter().enumerate() {
            assert_eq!(step.index(), index, "{step:?} appears more than once");
        }
    }

    #[test]
    fn next_and_previous_walk_the_step_order() {
        let steps = OnboardingStep::ALL;
        assert_eq!(steps.first(), Some(&OnboardingStep::Appearance));
        assert_eq!(steps[0].previous(), None);
        assert_eq!(steps[steps.len() - 1].next(), None);
        for pair in steps.windows(2) {
            assert_eq!(pair[0].next(), Some(pair[1]));
            assert_eq!(pair[1].previous(), Some(pair[0]));
        }
    }
}
