pub(crate) mod components;
#[cfg(not(target_os = "android"))]
pub(crate) mod develop;
pub(crate) mod develop_viewport;
pub(crate) mod icons;
pub(crate) mod layout;
pub(crate) mod library;
pub(crate) mod onboarding;
pub(crate) mod preview;
pub(crate) mod settings;
pub(crate) mod sidebar;
pub(crate) mod theme;
pub(crate) mod top_bar;

/// Desktop-only “save as” picker. The returned path always carries `extensions[0]`, so the
/// encoder downstream of it never sees a file it cannot open.
#[cfg(not(target_os = "android"))]
pub(crate) fn choose_save_path(
    filter: String,
    extensions: &'static [&'static str],
    default_name: &str,
    initial_directory: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .add_filter(filter, extensions)
        .set_file_name(default_name);
    if let Some(directory) = initial_directory.filter(|path| !path.as_os_str().is_empty()) {
        dialog = dialog.set_directory(directory);
    }
    let fallback_extension = extensions.first()?;
    let mut path = dialog.save_file()?;
    let valid_extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        });
    if !valid_extension {
        path.set_extension(fallback_extension);
    }
    Some(path)
}

/// Save picker for a developed-image export.
#[cfg(not(target_os = "android"))]
pub(crate) fn choose_export_file_path(
    format: crate::pipeline::ExportFormat,
    default_name: &str,
    initial_directory: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    choose_save_path(
        format!("{} image", format.label()),
        format.extensions(),
        default_name,
        initial_directory,
    )
}

/// Save picker for the Edit Replay MP4.
#[cfg(not(target_os = "android"))]
pub(crate) fn choose_edit_replay_file_path(
    default_name: &str,
    initial_directory: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    choose_save_path(
        "MP4 video".to_string(),
        MP4_EXTENSIONS,
        default_name,
        initial_directory,
    )
}

/// Extension list for [`choose_edit_replay_file_path`]; also the container FFmpeg is asked for.
#[cfg(not(target_os = "android"))]
const MP4_EXTENSIONS: &[&str] = &["mp4"];

#[cfg(target_os = "android")]
pub(crate) fn android_overflow_menu<R>(
    ui: &mut eframe::egui::Ui,
    anchor_rect: eframe::egui::Rect,
    id: eframe::egui::Id,
    edge: f32,
    add_contents: impl FnOnce(&mut eframe::egui::Ui) -> R,
) -> eframe::egui::Response {
    moduwu_design::overflow_menu(
        ui,
        anchor_rect,
        id,
        edge,
        egui_phosphor::regular::DOTS_THREE_VERTICAL,
        "More actions",
        add_contents,
    )
}

pub(crate) fn mask_component_color(index: usize) -> eframe::egui::Color32 {
    use crate::ui::theme::MASK_COMPONENT_COLORS;
    MASK_COMPONENT_COLORS[index % MASK_COMPONENT_COLORS.len()]
}
