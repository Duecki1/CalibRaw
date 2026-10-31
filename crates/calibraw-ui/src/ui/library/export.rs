use super::*;
use crate::i18n::tr;
use crate::ui::components::adjustment_slider::AdjustmentSlider;

pub(super) fn show_library_export_settings_controls(
    ui: &mut Ui,
    format: &mut ExportFormat,
    settings: &mut ExportSettings,
    watermark: crate::app::WatermarkView,
) -> crate::ui::sidebar::ExportControlsChange {
    crate::ui::sidebar::export_settings_controls(ui, format, settings, None, watermark)
}

/// The preset and LUT a batch export applies to every photo.
pub(super) fn show_export_override_controls(
    ui: &mut Ui,
    choice: &mut crate::app::ExportOverrideChoice,
    presets: &[crate::presets::StoredPreset],
    luts: &[crate::luts::LutEntry],
) {
    if presets.is_empty() && luts.is_empty() {
        return;
    }
    const FIELD_WIDTH: f32 = 220.0;
    moduwu_design::section_card(ui, tr("Apply to every photo"), |ui| {
        if !presets.is_empty() {
            let selected = choice
                .preset
                .as_deref()
                .and_then(|path| presets.iter().find(|stored| stored.path == path))
                .map_or_else(|| tr("None").to_owned(), preset_label);
            moduwu_design::form_combo_with_help(
                ui,
                tr("Preset"),
                "library-export-preset",
                selected,
                FIELD_WIDTH,
                tr("Applied on top of each photo's edit without changing it. Camera profile, lens correction and AI masks are left out."),
                |ui| {
                    ui.selectable_value(&mut choice.preset, None, tr("None"));
                    for stored in presets {
                        ui.selectable_value(
                            &mut choice.preset,
                            Some(stored.path.clone()),
                            preset_label(stored),
                        );
                    }
                },
            );
        }
        if !luts.is_empty() {
            let selected = choice
                .look_lut
                .and_then(|id| luts.iter().find(|entry| entry.id == id))
                .map_or(tr("None"), |entry| entry.name.as_str());
            moduwu_design::form_combo_with_help(
                ui,
                tr("LUT"),
                "library-export-lut",
                selected,
                FIELD_WIDTH,
                tr("Replaces each photo's LUT without changing its edit."),
                |ui| {
                    ui.selectable_value(&mut choice.look_lut, None, tr("None"));
                    for entry in luts {
                        ui.selectable_value(&mut choice.look_lut, Some(entry.id), &entry.name);
                    }
                },
            );
            if choice.look_lut.is_some() {
                let mut percent = choice.look_amount * 100.0;
                if AdjustmentSlider::new(tr("LUT amount"), &mut percent, 0.0..=100.0)
                    .decimals(0)
                    .step(1.0)
                    .reset_to(crate::luts::LookLut::DEFAULT_AMOUNT * 100.0)
                    .show(ui)
                {
                    choice.look_amount = percent / 100.0;
                }
            }
        }
    });
}

fn preset_label(stored: &crate::presets::StoredPreset) -> String {
    format!("{} — {}", stored.preset.group(), stored.preset.name())
}

/// The templated export name stem for `source`, without an extension.
pub(super) fn library_export_stem(source: &Path, name_template: &str) -> String {
    let original_name = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("calibraw-export");
    let context = crate::export_naming::ExportNameContext::from_display_metadata(
        original_name,
        load_raw_display_metadata(source).ok(),
        crate::export_naming::edited_time_for_path(source),
    );
    crate::export_naming::render_export_stem_or_default(name_template, &context)
}

pub(super) fn library_export_jobs(
    paths: &[PathBuf],
    format: ExportFormat,
    name_template: &str,
) -> Option<Vec<(PathBuf, LibraryExportDestination)>> {
    if paths.is_empty() {
        return None;
    }
    if paths.len() == 1 {
        let source = &paths[0];
        let stem = library_export_stem(source, name_template);
        let default_name =
            crate::export_naming::free_export_file_name(source.parent(), &stem, format.extension());
        let destination =
            crate::ui::choose_export_file_path(format, &default_name, source.parent())?;
        return Some(vec![(
            source.clone(),
            LibraryExportDestination::Chosen(destination),
        )]);
    }

    let mut dialog = crate::platform::FileDialog::new();
    if let Some(parent) = paths
        .first()
        .and_then(|path| path.parent())
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        dialog = dialog.set_directory(parent);
    }
    let folder = dialog.pick_folder()?;
    // Names are numbered when each finished file is moved into place, so
    // photos sharing a stem and files that appear meanwhile both get the
    // next free name.
    Some(
        paths
            .iter()
            .map(|source| {
                let destination = LibraryExportDestination::InFolder {
                    folder: folder.clone(),
                    stem: library_export_stem(source, name_template),
                };
                (source.clone(), destination)
            })
            .collect(),
    )
}
