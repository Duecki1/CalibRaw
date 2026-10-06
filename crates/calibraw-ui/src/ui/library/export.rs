use super::*;

pub(super) fn show_library_export_settings_controls(
    ui: &mut Ui,
    format: &mut ExportFormat,
    settings: &mut ExportSettings,
    picker_directory: Option<&Path>,
) -> bool {
    crate::ui::sidebar::export_settings_controls(ui, format, settings, picker_directory)
}

/// The templated export name stem for `source`, without an extension.
#[cfg(not(target_os = "android"))]
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

#[cfg(not(target_os = "android"))]
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

    let mut dialog = rfd::FileDialog::new();
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
