use crate::app::maximum_raw_cache_limit;
use crate::app::OnnxRuntimeMode;
use crate::app::{CalibRawApp, PreviewQuality, GITHUB_UPDATE_CHECKS_AVAILABLE, UPDATE_STORE_NAME};
use crate::pipeline::CameraProfileMode;
use crate::ui::components::adjustment_slider::AdjustmentSlider;
use crate::ui::library::maximum_thumbnail_worker_count;
use eframe::egui::{self, Ui};
use moduwu_design::ScreenLayout;

const PROJECT_LICENSE: &str = include_str!("../../../../COPYING");
const PROJECT_LICENSE_ID: &str = env!("CARGO_PKG_LICENSE");
const PROJECT_REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const THIRD_PARTY_NOTICES: &str = include_str!("../../../../THIRD_PARTY_NOTICES.md");
const RUST_DEPENDENCY_LICENSES: &str =
    include_str!(concat!(env!("OUT_DIR"), "/THIRD_PARTY_LICENSES.md"));

mod card_order;
mod lut_library;

pub(crate) struct Settings;

/// A count setting as a standard slider. Dragging or typing edits a draft that
/// is applied once the pointer is released and the value field loses focus,
/// because applying a change restarts work such as the thumbnail queue.
/// Returns whether the value was applied.
fn count_setting(
    ui: &mut Ui,
    label: &str,
    value: &mut usize,
    range: std::ops::RangeInclusive<usize>,
    default: usize,
    help: &str,
) -> bool {
    let draft_id = ui.make_persistent_id(("count-setting-draft", label));
    let mut draft = ui
        .data(|data| data.get_temp::<usize>(draft_id))
        .unwrap_or(*value);
    AdjustmentSlider::new(label, &mut draft, range)
        .reset_to(default)
        .show(ui);
    ui.small(format!("{help} Default: {default}."));

    // Only an edited draft is kept, so a value changed elsewhere is not
    // overwritten by a stale copy.
    if draft == *value {
        ui.data_mut(|data| data.remove::<usize>(draft_id));
        return false;
    }
    let editing =
        ui.input(|input| input.pointer.any_down()) || ui.ctx().egui_wants_keyboard_input();
    if editing {
        ui.data_mut(|data| data.insert_temp(draft_id, draft));
        return false;
    }
    ui.data_mut(|data| data.remove::<usize>(draft_id));
    *value = draft;
    true
}

fn diagnostics_snapshot_with_ai_backends() -> String {
    let mut diagnostic_log = calibraw_core::diagnostics::snapshot();
    let statuses = calibraw_ai::ai_session_statuses();
    if statuses.is_empty() {
        return diagnostic_log;
    }

    if !diagnostic_log.ends_with('\n') && !diagnostic_log.is_empty() {
        diagnostic_log.push('\n');
    }
    diagnostic_log.push_str("AI sessions (current):\n");
    for status in statuses {
        diagnostic_log.push_str("- ");
        diagnostic_log.push_str(&status.model_name);
        diagnostic_log.push_str(": ");
        diagnostic_log.push_str(&status.label());
        diagnostic_log.push('\n');
    }
    diagnostic_log
}

/// Who each subject-mask quality offered on this platform is for, one line
/// each, so they can be compared at once in setup and in the tooltip.
pub(crate) fn subject_mask_quality_descriptions() -> String {
    use calibraw_ai::models::subject::SubjectMaskQuality;
    SubjectMaskQuality::AVAILABLE
        .iter()
        .map(|&quality| {
            let description = match quality {
                SubjectMaskQuality::Low => "For older laptops and phones · fastest, softer edges",
                SubjectMaskQuality::Medium => "For most devices · recommended",
                SubjectMaskQuality::High => {
                    "For computers with a dedicated GPU · finest edges, about twice as long"
                }
            };
            format!("{}: {description}", quality.label())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Subject-mask quality, in setup, in the AI card on desktop and in the
/// interface card on Android. Locked while an AI mask is being generated.
pub(crate) fn subject_mask_quality_control(
    ui: &mut Ui,
    app: &mut CalibRawApp,
    id_salt: &'static str,
) {
    use calibraw_ai::models::subject::SubjectMaskQuality;
    let help = format!(
        "Applies to newly generated Subject and Background masks.\n{}",
        subject_mask_quality_descriptions()
    );
    let mut quality = app.ai.subject_mask_quality;
    ui.add_enabled_ui(app.ai_mask_generation_idle(), |ui| {
        moduwu_design::form_combo_with_help(
            ui,
            "Subject mask quality",
            id_salt,
            quality.label(),
            160.0,
            &help,
            |ui| {
                for &option in SubjectMaskQuality::AVAILABLE {
                    ui.selectable_value(&mut quality, option, option.label());
                }
            },
        );
    });
    app.set_subject_mask_quality(quality);
}

pub(crate) fn export_name_template_controls(
    ui: &mut Ui,
    app: &mut CalibRawApp,
    id_salt: &'static str,
) {
    ui.label("Name template");
    let changed = ui
        .add(
            moduwu_design::singleline_text_edit(&mut app.preferences.export_name_template)
                .id_salt(id_salt)
                .char_limit(crate::export_naming::MAX_EXPORT_NAME_TEMPLATE_CHARS)
                .desired_width(f32::INFINITY)
                .hint_text(crate::export_naming::DEFAULT_EXPORT_NAME_TEMPLATE),
        )
        .on_hover_text("Enter literal text and any of the placeholders listed below.")
        .changed();
    if changed {
        app.persist_performance_settings();
    }

    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(
                "{OriginalName}  {CurrentDate}  {EditedDate}  {ISO}  {ShutterSpeed}  {FocalLength}",
            )
            .monospace()
            .color(ui.visuals().weak_text_color()),
        );
    });
    ui.small(
        "Dates use YYYY-MM-DD. EditedDate uses the last saved edit or closest available file date, then today as a fallback. Missing capture metadata becomes “unknown”.",
    );

    let original_name = app
        .develop
        .current_label
        .as_deref()
        .and_then(|label| std::path::Path::new(label).file_stem())
        .and_then(std::ffi::OsStr::to_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("IMG_1234");
    let edited_at = app
        .develop
        .current_path
        .as_deref()
        .filter(|_| app.caps().has_folders())
        .and_then(crate::export_naming::edited_time_for_path);
    let context = if let Some(raw) = app.develop.loaded_raw.as_deref() {
        crate::export_naming::ExportNameContext::from_raw(original_name, raw, edited_at)
    } else {
        crate::export_naming::ExportNameContext::from_display_metadata(
            original_name,
            None,
            edited_at,
        )
    };
    match crate::export_naming::render_export_stem(&app.preferences.export_name_template, &context)
    {
        Ok(preview) => {
            ui.label(
                egui::RichText::new(format!("Example: {preview}.jpg"))
                    .color(ui.visuals().weak_text_color()),
            );
        }
        Err(error) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }
    if app.preferences.export_name_template != crate::export_naming::DEFAULT_EXPORT_NAME_TEMPLATE
        && moduwu_design::secondary_button(ui, "Reset to default").clicked()
    {
        app.preferences.export_name_template =
            crate::export_naming::DEFAULT_EXPORT_NAME_TEMPLATE.to_owned();
        app.persist_performance_settings();
    }
}

impl Settings {
    pub(crate) fn show(ui: &mut Ui, app: &mut CalibRawApp, layout: ScreenLayout) {
        let available_width = ui.available_width().max(1.0);
        let content_width = match layout {
            ScreenLayout::Horizontal => available_width.min(760.0),
            ScreenLayout::Vertical => available_width,
        }
        .max(1.0);
        let left_margin = ((available_width - content_width) * 0.5).max(0.0);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(left_margin);
            ui.vertical(|ui| {
                ui.set_width(content_width);
                ui.set_max_width(content_width);
                Self::show_content(ui, app, layout);
            });
        });
    }

    fn show_content(ui: &mut Ui, app: &mut CalibRawApp, layout: ScreenLayout) {
        if app.caps().is_mobile() {
            moduwu_design::toolbar_row(ui, |ui| {
                if crate::ui::top_bar::TopBar::back_icon_button(
                    ui,
                    moduwu_design::toolbar_icon_size(),
                )
                .clicked()
                {
                    app.activate_tab(crate::app::AppTab::Library);
                }
            });
        }
        moduwu_design::card_gap(ui);

        moduwu_design::content_card(ui, |ui| Self::show_usage_card(ui, app));

        moduwu_design::card_gap(ui);

        moduwu_design::content_card(ui, |ui| Self::show_appearance_card(ui, app));

        moduwu_design::card_gap(ui);

        moduwu_design::content_card(ui, |ui| Self::show_interface_card(ui, app));
        Self::show_develop_card_order_dialog(ui.ctx(), app);

        moduwu_design::card_gap(ui);
        moduwu_design::content_card(ui, |ui| Self::show_adjustment_clipboard_card(ui, app));

        moduwu_design::card_gap(ui);
        moduwu_design::content_card(ui, |ui| Self::show_export_names_card(ui, app));

        if app.presets.is_available() {
            moduwu_design::card_gap(ui);
            moduwu_design::content_card(ui, |ui| Self::show_new_photo_preset_card(ui, app));
        }

        moduwu_design::card_gap(ui);
        moduwu_design::content_card(ui, |ui| Self::show_lens_correction_card(ui, app));

        moduwu_design::card_gap(ui);
        moduwu_design::content_card(ui, |ui| Self::show_raw_color_profiles_card(ui, app));

        if app.luts.is_available() || app.luts.failure().is_some() {
            moduwu_design::card_gap(ui);
            moduwu_design::content_card(ui, |ui| Self::show_luts_card(ui, app));
            Self::show_lut_library_dialog(ui.ctx(), app);
            Self::show_lut_rename_dialog(ui.ctx(), app);
            Self::show_lut_remove_confirmation(ui.ctx(), app);
        }

        if app.caps().configurable_ai_runtime {
            moduwu_design::card_gap(ui);
            moduwu_design::content_card(ui, |ui| Self::show_ai_models_card(ui, app));
        }

        moduwu_design::card_gap(ui);
        moduwu_design::content_card(ui, |ui| Self::show_updates_card(ui, app));

        moduwu_design::card_gap(ui);
        moduwu_design::content_card(ui, |ui| Self::show_legal_card(ui, app));

        moduwu_design::card_gap(ui);
        moduwu_design::content_card(ui, |ui| Self::show_diagnostics_card(ui, app, layout));
    }

    fn show_usage_card(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::heading_with_help(
            ui,
            "Usage",
            "Shows the accumulated time CalibRaw has been running across sessions.",
        );
        ui.strong(crate::app::format_usage_duration(app.app_usage_duration()));
        ui.small("Total app use");
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
    }

    fn show_appearance_card(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::heading_with_help(
            ui,
            "Appearance",
            "Choose the design used across every screen and the canvas color shown around the photo. Changes are saved and applied immediately.",
        );

        let mut design = app.preferences.ui_design;
        moduwu_design::form_combo_with_help(
            ui,
            "Design",
            "settings-ui-design",
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

        let mut backdrop = app.preferences.preview_backdrop;
        let backdrop_help = match backdrop {
            crate::appearance::PreviewBackdrop::MatchPhoto => {
                "Derives a quiet, low-contrast canvas color from each image without competing with the edit. Toolbars, sidebars, and panels keep the selected design."
            }
            _ => {
                "Changes only the canvas around the photo. Toolbars, sidebars, and panels keep the selected design."
            }
        };
        moduwu_design::form_combo_with_help(
            ui,
            "Preview background",
            "settings-preview-backdrop",
            backdrop.label(),
            220.0,
            backdrop_help,
            |ui| {
                for option in crate::appearance::PreviewBackdrop::ALL {
                    ui.selectable_value(&mut backdrop, option, option.label());
                }
            },
        );
        if backdrop != app.preferences.preview_backdrop {
            app.set_preview_backdrop(backdrop);
        }
    }

    fn show_interface_card(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::heading_with_help(
            ui,
            "Interface",
            "Configure preview resolution, brush behavior, and library resource use.",
        );

        if moduwu_design::toggle_with_help(
            ui,
            &mut app.preferences.show_develop_navigation_labels,
            "Show Develop navigation labels",
            "Shows a short name below each icon in the Develop navigation bars. Turn this off for a quieter, more compact interface.",
        )
        .changed()
        {
            app.persist_performance_settings();
        }

        Self::show_develop_card_order(ui, app);

        let mut tips_enabled = app.preferences.tips.enabled;
        if moduwu_design::toggle_with_help(
            ui,
            &mut tips_enabled,
            "Show feature tips",
            "Shows a short animated tip the first time you use a feature, such as the Masks tab or the object mask.",
        )
        .changed()
        {
            app.set_tips_enabled(tips_enabled);
        }
        if moduwu_design::secondary_button_enabled(
            ui,
            !app.preferences.tips.seen.is_empty(),
            "Show seen tips again",
        )
        .clicked()
        {
            app.reset_seen_tips();
        }

        if app.discord_rich_presence_supported() {
            let mut enabled = app.preferences.discord_rich_presence;
            if moduwu_design::toggle_with_help(
                ui,
                &mut enabled,
                "Discord Rich Presence",
                "Shares only whether you are browsing the Library or editing, plus the elapsed edit time. Photo names and paths are never sent. Discord's desktop client must be running.",
            )
            .changed()
            {
                app.set_discord_rich_presence(enabled);
            }
            if !app.discord_rich_presence_configured() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Unavailable in this build: CALIBRAW_DISCORD_APPLICATION_ID is not configured.",
                );
            }
        }

        let previous_quality = app.preview.quality;
        let quality_help = if app.caps().constrained_memory() {
            "Controls how many pixels the preview renders compared with the screen pixels it covers: Low 50%, Medium 75%, High 90%, and Max 100%, one rendered pixel per screen pixel. Zoom detail uses the same density. A device with little GPU memory may render fewer pixels than asked."
        } else {
            "Controls how many pixels the preview renders compared with the screen pixels it covers: Low 75%, Medium 100%, High 125%, and Max 150%. Above 100% the preview renders more pixels than the screen has and averages them down when scaled to it, which keeps the fitted view sharp. Zoom detail uses the same density."
        };
        moduwu_design::form_combo_with_help(
            ui,
            "Preview quality",
            "settings-preview-quality",
            app.preview.quality.label(),
            160.0,
            quality_help,
            |ui| {
                ui.selectable_value(&mut app.preview.quality, PreviewQuality::Low, "Low");
                ui.selectable_value(&mut app.preview.quality, PreviewQuality::Medium, "Medium");
                ui.selectable_value(&mut app.preview.quality, PreviewQuality::High, "High");
                ui.selectable_value(&mut app.preview.quality, PreviewQuality::Max, "Max");
            },
        );
        if app.preview.quality != previous_quality {
            app.preview_quality_changed();
        }
        if app.caps().full_resolution_preview() {
            let mut full_resolution = app.preview.full_resolution.enabled();
            if moduwu_design::toggle_with_help(
                ui,
                &mut full_resolution,
                "Full-resolution preview",
                "When nothing has changed for a moment, renders the whole photo from the original RAW instead of the reduced preview and shows it scaled to your screen. Editing and zooming stay on the fast preview. Needs extra GPU memory and a few seconds on large photos.",
            )
            .changed()
            {
                app.preview.full_resolution.set_enabled(full_resolution);
                app.persist_performance_settings();
            }
        }
        // Without a configurable AI runtime there is no AI models card to hold
        // this control.
        if !app.caps().configurable_ai_runtime {
            subject_mask_quality_control(ui, app, "settings-subject-mask-quality");
        }

        moduwu_design::section_separator(ui);
        if moduwu_design::toggle_with_help(
            ui,
            &mut app.preferences.image_relative_brush_size,
            "Keep brush size fixed to the image",
            "When enabled, the brush keeps the same image footprint at every zoom level. When disabled, it keeps the same on-screen footprint.",
        )
            .changed()
        {
            app.persist_performance_settings();
        }

        moduwu_design::section_separator(ui);
        moduwu_design::strong_with_help(
            ui,
            "Library performance",
            "Tune memory use and background preview generation. Conservative values are recommended on phones.",
        );
        if app.caps().has_folders() {
            let mut render_edited_thumbnails =
                app.library.renders_edited_thumbnails_during_indexing();
            if moduwu_design::toggle_with_help(
                ui,
                &mut render_edited_thumbnails,
                "Render edited thumbnails while indexing",
                "Applies saved edits to library thumbnails during indexing. Turning it off keeps indexing faster and memory use lower; existing edited previews are still reused.",
            )
                .changed()
            {
                app.set_render_edited_thumbnails_during_indexing(render_edited_thumbnails);
            }
        }

        if !app.caps().constrained_memory() {
            let mut raw_cache_files = app.develop.decoded_raws.limit();
            if count_setting(
                ui,
                "Decoded RAW cache (files)",
                &mut raw_cache_files,
                0..=maximum_raw_cache_limit(),
                crate::app::default_raw_cache_limit(),
                "Keeps decoded RAW files in memory for faster switching, including the current image. Files beyond the current one are used to prepare the next and previous photos in advance. Set to 0 to disable reuse; the current edit stays loaded.",
            ) {
                app.set_raw_cache_limit(raw_cache_files);
            }

            ui.add_space(moduwu_design::SPACE_SM);
            let mut hdd_mode = app.preferences.hdd_mode;
            if moduwu_design::toggle_with_help(
                ui,
                &mut hdd_mode,
                "HDD mode",
                "Reads one photo file at a time, which is faster for libraries on spinning hard drives. Thumbnail workers still decode in parallel. Leave it off for SSDs and memory cards.",
            )
            .changed()
            {
                app.set_hdd_mode(hdd_mode);
            }

            ui.add_space(moduwu_design::SPACE_SM);
            let mut automatic_workers = app.library.automatic_thumbnail_workers();
            let automatic_help = format!(
                "Uses {} concurrent thumbnail jobs on this computer, sized from its processor cores and memory. Turn off to set the number yourself. Changing this restarts the queue.",
                crate::ui::library::automatic_thumbnail_worker_count()
            );
            if moduwu_design::toggle_with_help(
                ui,
                &mut automatic_workers,
                "Automatic thumbnail workers",
                &automatic_help,
            )
            .changed()
            {
                app.set_automatic_thumbnail_workers(automatic_workers);
            }

            if !app.library.automatic_thumbnail_workers() {
                ui.add_space(moduwu_design::SPACE_SM);
                let mut thumbnail_workers = app.thumbnail_worker_count();
                if count_setting(
                    ui,
                    "Thumbnail workers",
                    &mut thumbnail_workers,
                    1..=maximum_thumbnail_worker_count(),
                    crate::ui::library::default_thumbnail_worker_count(),
                    "Concurrent thumbnail jobs. More workers can fill the library faster but use more memory, especially for edited RAW files or files without embedded previews. Changing this restarts the queue.",
                ) {
                    app.set_thumbnail_worker_count(thumbnail_workers);
                }
            }
        }

        moduwu_design::section_separator(ui);
        moduwu_design::strong_with_help(
            ui,
            "Thumbnail cache",
            "Delete generated library previews. CalibRaw rebuilds them from the RAW files and saved edits when needed.",
        );
        let cache_size = app.thumbnail_cache_size_label();
        moduwu_design::action_row(ui, |ui| {
            if moduwu_design::secondary_button(ui, "Clear thumbnail cache")
                .on_hover_text("Delete generated previews and rebuild them when needed.")
                .clicked()
            {
                app.clear_thumbnail_cache();
            }
            ui.label(egui::RichText::new(cache_size).color(ui.visuals().weak_text_color()));
        });
    }

    fn show_adjustment_clipboard_card(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::heading_with_help(
            ui,
            "Copy & paste adjustments",
            "Choose which categories Library > Copy Adjustments stores for later pasting. The selection is remembered across restarts.",
        );

        let mut settings = app.preferences.adjustment_copy_settings;
        let mut changed = false;
        changed |= moduwu_design::toggle_with_help(
            ui,
            &mut settings.adjustments,
            "Adjustments",
            "Global light, color, white-balance temperature/tint, tone curve, effects (including the effects added on the Adjustments tab), color mixer, and RAW adjustment values.",
        )
            .changed();
        changed |= moduwu_design::toggle_with_help(
            ui,
            &mut settings.geometry,
            "Geometry",
            "Crop, rotation, straighten, perspective, flips, and geometry transforms. Disabled by default.",
        )
            .changed();
        changed |= moduwu_design::toggle_with_help(
            ui,
            &mut settings.camera_profile,
            "Camera profile",
            "The per-image camera/DCP profile selection. Enabled by default.",
        )
        .changed();
        changed |= moduwu_design::toggle_with_help(
            ui,
            &mut settings.lut,
            "LUT",
            "The LUT and its amount. Enabled by default.",
        )
        .changed();
        changed |= moduwu_design::toggle_with_help(
            ui,
            &mut settings.masks,
            "Normal masks",
            "Brush, radial-gradient, and linear-gradient mask components, including local adjustments. Mixed groups are split so disabling this never copies their manual components.",
        )
            .changed();
        changed |= moduwu_design::toggle_with_help(
            ui,
            &mut settings.ai_masks,
            "AI masks",
            "Subject, background, sky, object, luminance-range, and color-range components. Source-dependent results are marked for regeneration on the destination image.",
        )
            .changed();
        changed |= moduwu_design::toggle_with_help(
            ui,
            &mut settings.lens_correction,
            "Lens correction",
            "Lens correction enabled state and selected lens profile.",
        )
        .changed();
        if changed {
            app.set_adjustment_copy_settings(settings);
        }
    }

    fn show_new_photo_preset_card(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::heading_with_help(
            ui,
            "New photos",
            "Choose a preset that is applied when you open a photo that has no saved edits. Photos you have already edited keep their edits, and the preset can be undone like any other edit.",
        );

        let current = app
            .new_photo_preset_path()
            .filter(|path| app.presets.get(path).is_some());
        let mut choice = current.clone();
        let selected = current
            .as_deref()
            .and_then(|path| app.presets.get(path))
            .map_or_else(|| "None".to_owned(), |preset| preset.name().to_owned());
        moduwu_design::form_combo_with_help(
            ui,
            "Default preset",
            "settings-new-photo-preset",
            selected,
            220.0,
            "Applied once, when the photo first opens in Develop.",
            |ui| {
                ui.selectable_value(&mut choice, None, "None");
                for stored in app.presets.all() {
                    ui.selectable_value(
                        &mut choice,
                        Some(stored.path.clone()),
                        format!("{} / {}", stored.preset.group(), stored.preset.name()),
                    );
                }
            },
        );
        if choice != current {
            app.set_new_photo_preset(
                choice.and_then(|path| path.file_name().map(std::path::PathBuf::from)),
            );
        }
    }

    fn show_lens_correction_card(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::heading_with_help(
            ui,
            "Lens correction",
            "Choose what happens when you open a photo that has no saved edits. Photos you have already edited keep their own lens settings, and you can always change them in the Lens Corrections card.",
        );

        let previous = app.preferences.automatic_lens_correction;
        let mut automatic = previous;
        let mut missing_profile_prompt = app.preferences.missing_lens_profile_prompt;
        moduwu_design::toggle_with_help(
            ui,
            &mut automatic.enabled,
            "Automatic lens correction",
            "Applies the Lensfun profile matched from the photo's RAW metadata when you open it. Enabled by default.",
        );
        ui.add_enabled_ui(automatic.enabled, |ui| {
            moduwu_design::toggle_with_help(
                ui,
                &mut automatic.corrections.geometry,
                "Geometry",
                "Corrects distortion and colour fringing (lateral chromatic aberration) automatically.",
            );
            moduwu_design::toggle_with_help(
                ui,
                &mut automatic.corrections.vignetting,
                "Vignetting",
                "Brightens the corners the lens darkens automatically.",
            );
            moduwu_design::toggle_with_help(
                ui,
                &mut missing_profile_prompt,
                "Ask when no profile matches",
                "Offers manual lens correction when a RAW opens without a matching Lensfun profile.",
            );
        });
        // At least one correction must stay selected.
        if !automatic.corrections.geometry && !automatic.corrections.vignetting {
            automatic.corrections = previous.corrections;
        }
        app.set_automatic_lens_correction(automatic);
        app.set_missing_lens_profile_prompt(missing_profile_prompt);
    }

    fn show_export_names_card(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::heading_with_help(
            ui,
            "Export file names",
            "Build export file names from the original name, dates, and capture metadata. The selected image format adds its extension automatically.",
        );
        export_name_template_controls(ui, app, "settings-export-name-template");
    }

    fn show_raw_color_profiles_card(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::heading_with_help(
            ui,
            "RAW color profiles",
            "Choose how CalibRaw builds the camera-to-working color transform and whether it applies DCP color-rendering stages.",
        );

        let previous_mode = app.preferences.camera_profile_mode;
        let mut mode = previous_mode;
        let profile_mode_help = match mode {
            CameraProfileMode::Automatic => {
                "Uses the RAW's embedded camera matrix by default. A DCP selected for an individual image remains explicit."
            }
            CameraProfileMode::DcpProfiles => {
                "Searches the configured folder for this camera. If no safe DCP match exists, CalibRaw falls back to the embedded camera matrix."
            }
            CameraProfileMode::MatrixOnly => {
                "Ignores DCP look tables, hue/saturation maps, and profile tone curves and uses the camera/DNG/LibRaw matrix path."
            }
        };
        moduwu_design::form_combo_with_help(
            ui,
            "Profile mode",
            "settings-camera-profile-mode",
            mode.label(),
            190.0,
            profile_mode_help,
            |ui| {
                ui.selectable_value(&mut mode, CameraProfileMode::Automatic, "Automatic");
                ui.selectable_value(
                    &mut mode,
                    CameraProfileMode::DcpProfiles,
                    "Use DCP profiles",
                );
                ui.selectable_value(
                    &mut mode,
                    CameraProfileMode::MatrixOnly,
                    "Embedded matrix only",
                );
            },
        );
        if mode != previous_mode {
            app.set_camera_profile_mode(mode);
        }

        moduwu_design::section_separator(ui);
        let has_folders = app.caps().has_folders();
        let camera_folder_help = if has_folders {
            "Choose a top-level DCP profile folder. CalibRaw searches subfolders recursively. Auto-detect checks common CameraProfiles locations on Windows and macOS."
        } else {
            "Choose a top-level CameraProfiles folder with Android's system picker. CalibRaw recursively imports only .dcp files into private persistent storage, groups matches by camera, and exposes them in Develop."
        };
        moduwu_design::strong_with_help(ui, "Camera profile folder", camera_folder_help);
        if let Some(label) = &app.android.camera_profile_folder_importing_label {
            ui.add(
                egui::Label::new(egui::RichText::new(format!("Importing {label}…")).strong())
                    .wrap(),
            );
        } else if let Some(path) = &app.preferences.camera_profile_folder {
            if has_folders {
                if let Some(label) = &app.preferences.camera_profile_folder_label {
                    ui.small(label);
                }
                ui.add(
                    egui::Label::new(egui::RichText::new(path.display().to_string()).monospace())
                        .wrap(),
                );
            } else {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            app.preferences
                                .camera_profile_folder_label
                                .as_deref()
                                .unwrap_or("CameraProfiles"),
                        )
                        .strong(),
                    )
                    .wrap(),
                );
            }
        } else {
            ui.small("No external DCP folder selected.");
        }
        moduwu_design::action_row(ui, |ui| {
            let choose_label = if app.android.camera_profile_folder_importing_label.is_some() {
                "Importing…"
            } else {
                "Choose folder…"
            };
            let choose_enabled = !app.android.picker_pending;

            if moduwu_design::secondary_button_enabled(ui, choose_enabled, choose_label)
                .on_hover_text(camera_folder_help)
                .clicked()
            {
                app.choose_camera_profile_folder();
            }
            if has_folders
                && moduwu_design::secondary_button(ui, "Auto-detect DCP profiles")
                    .on_hover_text("Check common CameraProfiles locations.")
                    .clicked()
            {
                app.auto_detect_camera_profile_folder();
            }
            let can_clear = (app.preferences.camera_profile_folder.is_some()
                || app.preferences.camera_profile_auto_detect)
                && !app.android.picker_pending;
            if moduwu_design::secondary_button_enabled(ui, can_clear, "Clear")
                .on_hover_text("Stop using the configured external DCP profile folder.")
                .clicked()
            {
                app.clear_camera_profile_folder();
            }
        });
    }

    fn show_lut_rename_dialog(ctx: &egui::Context, app: &mut CalibRawApp) {
        let Some(rename) = app.luts.rename.as_mut() else {
            return;
        };
        let mut choice = moduwu_design::DialogAction::None;
        let confirm_enabled = !rename.name.trim().is_empty();
        moduwu_design::Dialog::new("lut-rename-dialog", "Rename LUT").show_with_footer(
            ctx,
            |ui| {
                ui.label("LUT name");
                let response =
                    moduwu_design::dialog_text_field(ui, &mut rename.name, "lut-name", "e.g. Film");
                moduwu_design::request_initial_focus(&response, &mut rename.focus_requested);
            },
            |ui| {
                choice = moduwu_design::dialog_confirmation_actions(
                    ui,
                    "Cancel",
                    "Rename",
                    confirm_enabled,
                    false,
                    moduwu_design::DialogKeyboard::CONFIRM_ON_ENTER,
                );
            },
        );
        match choice {
            moduwu_design::DialogAction::Cancel => app.luts.rename = None,
            moduwu_design::DialogAction::Confirm => {
                if let Some(rename) = app.luts.rename.as_ref() {
                    let (id, name) = (rename.id, rename.name.clone());
                    app.rename_lut(id, &name);
                }
            }
            moduwu_design::DialogAction::None => {}
        }
    }

    fn show_lut_remove_confirmation(ctx: &egui::Context, app: &mut CalibRawApp) {
        let Some(id) = app.luts.pending_remove else {
            return;
        };
        let Some(name) = app.luts.entry(id).map(|entry| entry.name.clone()) else {
            app.luts.pending_remove = None;
            return;
        };
        let mut choice = moduwu_design::DialogAction::None;
        moduwu_design::Dialog::new("lut-remove-confirmation", "Remove LUT?").show_with_footer(
            ctx,
            |ui| {
                ui.add(
                    egui::Label::new(format!(
                        "“{name}” will be removed. Photos that use it keep naming it and are shown without it until it is imported again."
                    ))
                    .wrap(),
                );
            },
            |ui| {
                choice = moduwu_design::dialog_confirmation_actions(
                    ui,
                    "Cancel",
                    "Remove",
                    true,
                    true,
                    moduwu_design::DialogKeyboard::CLOSE_ONLY,
                );
            },
        );
        match choice {
            moduwu_design::DialogAction::Cancel => app.luts.pending_remove = None,
            moduwu_design::DialogAction::Confirm => app.remove_lut(id),
            moduwu_design::DialogAction::None => {}
        }
    }

    fn show_ai_models_card(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::heading_with_help(
            ui,
            "AI models",
            "Configure local inference acceleration, Subject mask quality, and the trusted ONNX Runtime library used by AI tools.",
        );
        let mut acceleration = app.ai.gpu_acceleration;
        if moduwu_design::toggle_with_help(
            ui,
            &mut acceleration,
            "Use GPU acceleration when available",
            "Allows AI masks and AI denoise to use a supported GPU execution provider. A model that does not fit in free GPU memory runs on the CPU, and CPU fallback remains automatic. Turn this off if GPU inference causes driver errors, instability, or incorrect results.",
        )
            .changed()
        {
            app.set_ai_gpu_acceleration(acceleration);
        }
        let statuses = calibraw_ai::ai_session_statuses();
        let mut in_use: Vec<String> = statuses.iter().map(|status| status.label()).collect();
        in_use.sort_unstable();
        in_use.dedup();
        if !in_use.is_empty() {
            let text = format!("In use: {}", in_use.join(", "));
            if statuses.iter().any(|status| status.gpu_failed()) {
                ui.colored_label(ui.visuals().warn_fg_color, text).on_hover_text(
                    "A GPU execution provider failed to start or run. The Diagnostics log lists the reason.",
                );
            } else {
                ui.small(text);
            }
        }

        moduwu_design::section_separator(ui);
        subject_mask_quality_control(ui, app, "settings-subject-mask-quality");

        moduwu_design::section_separator(ui);
        let bundled_runtime = calibraw_ai::runtime::bundled_onnx_runtime_path();
        let automatic_help = if bundled_runtime.is_some() {
            "Recommended. Use the ONNX Runtime included with CalibRaw, with CPU AI fallback and no runtime download."
        } else {
            "Recommended. Download and use CalibRaw's verified runtime for this platform when AI is first used."
        };
        let runtime_help = if bundled_runtime.is_some() {
            "Automatic uses the ONNX Runtime included with CalibRaw. AI tools can fall back to CPU inference. Manual uses a local shared-library override."
        } else {
            "Automatic downloads the verified ONNX Runtime package matching this operating system and CPU architecture when an AI tool first needs it. Manual uses a local shared-library override."
        };
        moduwu_design::strong_with_help(ui, "ONNX Runtime", runtime_help);
        let previous_mode = app.ai.runtime_mode;
        let mut runtime_mode = previous_mode;
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut runtime_mode,
                OnnxRuntimeMode::Automatic,
                "Automatic",
            )
            .on_hover_text(automatic_help);
            ui.selectable_value(
                &mut runtime_mode,
                OnnxRuntimeMode::Manual,
                "Manual",
            )
            .on_hover_text("Override automatic selection with a trusted local ONNX Runtime shared library.");
        });
        if runtime_mode != previous_mode {
            app.set_onnx_runtime_mode(runtime_mode);
        }

        match app.ai.runtime_mode {
            OnnxRuntimeMode::Automatic => {
                if let Some(path) = bundled_runtime {
                    if let Some(runtime) = calibraw_ai::runtime::automatic_onnx_runtime_info() {
                        ui.label(format!(
                            "ONNX Runtime {} is included with CalibRaw. No runtime download is needed.",
                            runtime.version
                        ));
                    }
                    ui.small("AI tools can fall back to CPU inference. AI models are downloaded separately when needed.");
                    if !calibraw_ai::runtime::automatic_onnx_runtime_is_installed() {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            format!(
                                "The included runtime is missing: {}. Repair the CalibRaw installation or select a trusted manual runtime.",
                                path.display()
                            ),
                        );
                    }
                } else {
                    ui.label(format!(
                        "CalibRaw will select the {} / {} runtime and download it when an AI tool first needs it.",
                        std::env::consts::OS,
                        std::env::consts::ARCH
                    ));
                    ui.small("The archive and extracted runtime are cached locally and verified with pinned SHA-256 values.");
                    if let Some(url) = calibraw_ai::runtime::automatic_onnx_runtime_info()
                        .and_then(|runtime| runtime.download_url)
                    {
                        ui.hyperlink_to("View runtime source", url);
                    }
                }
            }
            OnnxRuntimeMode::Manual => {
                let manual_help = if crate::platform::ONNX_RUNTIME_IS_DLL {
                    "Choose a trusted ONNX Runtime 1.22 or newer onnxruntime.dll matching this CalibRaw build's CPU architecture. CalibRaw validates it in an isolated helper process. AI uses the GPU only if WebGPU is built into it. Dependencies must remain beside it."
                } else {
                    "Choose a trusted ONNX Runtime 1.22 or newer shared library built for this hardware. CalibRaw validates it in an isolated helper process. AI uses the GPU only if WebGPU is built into it. Dependencies must remain beside it."
                };
                if let Some((path, sha256)) = &app.ai.manual_runtime {
                    ui.label("Selected manual runtime:");
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(path.display().to_string()).monospace(),
                        )
                        .wrap(),
                    );
                    ui.small("Pinned SHA-256:");
                    ui.add(
                        egui::Label::new(egui::RichText::new(sha256.as_str()).monospace()).wrap(),
                    );
                } else {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "No manual runtime selected. AI tools need a file override or Automatic mode.",
                    );
                }
                moduwu_design::action_row(ui, |ui| {
                    if moduwu_design::secondary_button(ui, "Choose ONNX Runtime…")
                        .on_hover_text(manual_help)
                        .clicked()
                    {
                        app.choose_onnx_runtime();
                    }
                    if moduwu_design::secondary_button_enabled(
                        ui,
                        app.ai.manual_runtime.is_some(),
                        "Clear",
                    )
                    .on_hover_text("Forget the selected manual runtime.")
                    .clicked()
                    {
                        app.clear_onnx_runtime();
                    }
                });
            }
        }
    }

    fn show_updates_card(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::heading_with_help(
            ui,
            "Updates",
            "View the installed version and optionally ask GitHub for CalibRaw's latest stable release.",
        );
        ui.strong(format!("CalibRaw {}", env!("CARGO_PKG_VERSION")));
        if !GITHUB_UPDATE_CHECKS_AVAILABLE {
            ui.small(format!(
                "Updates are delivered through {UPDATE_STORE_NAME}."
            ));
            return;
        }

        moduwu_design::section_separator(ui);
        let mut auto_check = app.preferences.auto_check_updates;
        let permission_denied = app.version_check_permission_denied();
        ui.add_enabled_ui(!permission_denied, |ui| {
            if moduwu_design::toggle_with_help(
                ui,
                &mut auto_check,
                "Automatically check for updates via GitHub",
                "Requires GitHub version-check permission. When enabled, CalibRaw checks once at startup. The connection exposes its public IP address and sends CalibRaw/<version> as its User-Agent; CalibRaw sends no photos, paths, account identifier, or analytics/telemetry.",
            )
            .changed()
            {
                app.set_auto_check_updates(auto_check);
            }
        });
        if permission_denied {
            ui.small(
                "Automatic and manual GitHub checks are disabled. Use ‘Review privacy & permission’ to explicitly change this choice.",
            );
        }

        ui.small(format!(
            "GitHub permission: {}",
            app.version_check_permission_text()
        ));
        moduwu_design::action_row(ui, |ui| {
            if moduwu_design::secondary_button(ui, "Review privacy & permission").clicked() {
                app.review_version_check_privacy();
            }
            if moduwu_design::secondary_button_enabled(
                ui,
                app.version_check_permission_granted(),
                "Revoke permission",
            )
            .on_hover_text("Stops future GitHub version checks and disables automatic checks.")
            .clicked()
            {
                app.revoke_version_check_permission();
            }
        });

        let checking = app.version_check_in_progress();
        let status = app.version_check_status_text();
        moduwu_design::action_row(ui, |ui| {
            if moduwu_design::secondary_button_enabled(
                ui,
                !checking && !permission_denied,
                "Check now",
            )
                .on_hover_text(if permission_denied {
                    "GitHub checks are blocked because permission was declined. Use ‘Review privacy & permission’ to change that choice first."
                } else {
                    "Check GitHub for the latest stable CalibRaw release. If permission has not been decided yet, CalibRaw asks first."
                })
                .clicked()
            {
                app.check_for_updates(true);
            }
            ui.hyperlink_to("View releases", format!("{PROJECT_REPOSITORY}/releases"));
        });
        ui.small(status);
    }

    fn show_legal_card(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::heading_with_help(
            ui,
            "Legal & attributions",
            "Adapted code, bundled data, native libraries, optional AI models, Rust crates, fonts, and icons retain their listed upstream terms and notices.",
        );
        ui.strong(format!("CalibRaw {}", env!("CARGO_PKG_VERSION")));
        ui.label("Copyright (C) 2026 Duecki and CalibRaw contributors");
        ui.label(format!("Licensed under {PROJECT_LICENSE_ID}."));
        ui.hyperlink_to("Project source and license", PROJECT_REPOSITORY);

        moduwu_design::action_row(ui, |ui| {
            if moduwu_design::secondary_button(ui, "Copy all legal text")
                .on_hover_text("Copy the project license and every bundled third-party notice.")
                .clicked()
            {
                let legal_text = format!(
                    "{PROJECT_LICENSE}\n\n{THIRD_PARTY_NOTICES}\n\n{RUST_DEPENDENCY_LICENSES}"
                );
                match app.copy_text_to_clipboard("CalibRaw legal notices", &legal_text) {
                    None => ui.ctx().copy_text(legal_text),
                    Some(Ok(())) => calibraw_core::diagnostics::record(
                        "CalibRaw legal notices copied to Android clipboard",
                    ),
                    Some(Err(error)) => calibraw_core::diagnostics::record(format!(
                        "Android legal-notice clipboard copy failed: {error}"
                    )),
                }
            }
        });

        egui::CollapsingHeader::new("GNU GPL v3 or later")
            .default_open(false)
            .show(ui, |ui| Self::legal_text(ui, PROJECT_LICENSE, 16));
        egui::CollapsingHeader::new("Project and third-party notices")
            .default_open(false)
            .show(ui, |ui| Self::legal_text(ui, THIRD_PARTY_NOTICES, 18));
        egui::CollapsingHeader::new("Rust dependencies, fonts, and icons")
            .default_open(false)
            .show(ui, |ui| Self::legal_text(ui, RUST_DEPENDENCY_LICENSES, 18));
    }

    fn show_diagnostics_card(ui: &mut Ui, app: &mut CalibRawApp, layout: ScreenLayout) {
        moduwu_design::heading_with_help(
            ui,
            "Diagnostics",
            "For comparisons, open the same RAW and run an export on each device, then copy the log. It includes platform, GPU/backend, RAW calibration, fingerprints, and timing information.",
        );

        let mut diagnostic_log = diagnostics_snapshot_with_ai_backends();
        moduwu_design::action_row(ui, |ui| {
            if moduwu_design::secondary_button(ui, "Copy log")
                .on_hover_text("Copy the complete diagnostic report.")
                .clicked()
            {
                match app.copy_text_to_clipboard("CalibRaw diagnostics", &diagnostic_log) {
                    None => ui.ctx().copy_text(diagnostic_log.clone()),
                    Some(Ok(())) => calibraw_core::diagnostics::record(
                        "Diagnostic log copied to Android clipboard",
                    ),
                    Some(Err(error)) => calibraw_core::diagnostics::record(format!(
                        "Android clipboard copy failed: {error}"
                    )),
                }
            }
            if moduwu_design::secondary_button(ui, "Clear events")
                .on_hover_text("Clear recorded runtime events from the diagnostic report.")
                .clicked()
            {
                calibraw_core::diagnostics::clear();
                diagnostic_log = diagnostics_snapshot_with_ai_backends();
            }
        });

        let rows = match layout {
            ScreenLayout::Horizontal => 16,
            ScreenLayout::Vertical => 10,
        };
        let mut diagnostic_view = diagnostic_log.as_str();
        egui::CollapsingHeader::new("View diagnostic log")
            .default_open(false)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut diagnostic_view)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false)
                        .desired_rows(rows)
                        .desired_width(f32::INFINITY),
                );
            });
    }

    fn legal_text(ui: &mut Ui, text: &str, rows: usize) {
        let mut view = text;
        ui.add(
            egui::TextEdit::multiline(&mut view)
                .font(egui::TextStyle::Monospace)
                .interactive(false)
                .desired_rows(rows)
                .desired_width(f32::INFINITY),
        );
    }
}
