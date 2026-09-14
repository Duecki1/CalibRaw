use super::*;

pub(super) fn show_library_export_settings_controls(
    ui: &mut Ui,
    format: &mut ExportFormat,
    settings: &mut ExportSettings,
    picker_directory: Option<&Path>,
) -> bool {
    crate::ui::sidebar::export_settings_controls(ui, format, settings, picker_directory)
}

#[cfg(not(target_os = "android"))]
pub(super) fn unique_library_export_path(
    folder: &Path,
    source: &Path,
    format: ExportFormat,
    name_template: &str,
    reserved: &mut HashSet<PathBuf>,
) -> PathBuf {
    let original_name = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("calibraw-export");
    let metadata = load_raw_display_metadata(source).ok();
    let edited_at = crate::export_naming::edited_time_for_path(source);
    let context = crate::export_naming::ExportNameContext::from_display_metadata(
        original_name,
        metadata,
        edited_at,
    );
    let base = crate::export_naming::render_export_stem_or_default(name_template, &context);
    let mut index = 1usize;
    loop {
        let name = if index == 1 {
            format!("{base}.{}", format.extension())
        } else {
            format!("{base}-{index}.{}", format.extension())
        };
        let candidate = folder.join(name);
        if !candidate.exists() && reserved.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

#[cfg(not(target_os = "android"))]
pub(super) fn library_export_jobs(
    paths: &[PathBuf],
    format: ExportFormat,
    name_template: &str,
) -> Option<Vec<(PathBuf, PathBuf)>> {
    if paths.is_empty() {
        return None;
    }
    if paths.len() == 1 {
        let source = &paths[0];
        let original_name = source
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("calibraw-export");
        let context = crate::export_naming::ExportNameContext::from_display_metadata(
            original_name,
            load_raw_display_metadata(source).ok(),
            crate::export_naming::edited_time_for_path(source),
        );
        let stem = crate::export_naming::render_export_stem_or_default(name_template, &context);
        let default_name = format!("{stem}.{}", format.extension());
        let destination =
            crate::ui::choose_export_file_path(format, &default_name, source.parent())?;
        return Some(vec![(source.clone(), destination)]);
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
    let mut reserved = HashSet::new();
    Some(
        paths
            .iter()
            .map(|source| {
                let destination = unique_library_export_path(
                    &folder,
                    source,
                    format,
                    name_template,
                    &mut reserved,
                );
                (source.clone(), destination)
            })
            .collect(),
    )
}
