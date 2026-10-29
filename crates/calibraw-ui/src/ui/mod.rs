pub(crate) mod background_tasks;
pub(crate) mod components;
pub(crate) mod develop;
pub(crate) mod develop_viewport;
pub(crate) mod icons;
pub(crate) mod layout;
pub(crate) mod library;
pub(crate) mod manual_lens_dialog;
pub(crate) mod onboarding;
pub(crate) mod presets;
pub(crate) mod preview;
pub(crate) mod settings;
pub(crate) mod shortcuts_dialog;
pub(crate) mod sidebar;
pub(crate) mod theme;
pub(crate) mod tips;
pub(crate) mod top_bar;

use crate::i18n::{tr, tr_fmt};

/// The native “save as” picker, where storage is folders. Native paths receive a supported extension when needed;
/// Flatpak paths keep the exact filename granted by the document portal.
pub(crate) fn choose_save_path(
    filter: String,
    extensions: &'static [&'static str],
    default_name: &str,
    initial_directory: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    extensions.first()?;
    let is_flatpak = crate::platform::is_flatpak();
    let mut file_name = default_name.to_owned();
    let mut directory = initial_directory
        .filter(|path| !path.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf);
    loop {
        let mut dialog = crate::platform::FileDialog::new()
            .add_filter(filter.clone(), extensions)
            .set_file_name(&file_name);
        if let Some(directory) = &directory {
            dialog = dialog.set_directory(directory);
        }
        let selected_path = dialog.save_file()?;
        let path = save_path_with_extension(selected_path.clone(), extensions, is_flatpak)?;
        if is_flatpak || path == selected_path || !path.exists() {
            return Some(path);
        }
        // The dialog only checked the name without the extension. Offer the
        // completed name again so the platform asks before replacing it.
        file_name = path.file_name()?.to_string_lossy().into_owned();
        directory = path.parent().map(std::path::Path::to_path_buf);
    }
}

/// Apply native extension defaults without changing Flatpak document grants.
fn save_path_with_extension(
    mut path: std::path::PathBuf,
    extensions: &[&str],
    is_flatpak: bool,
) -> Option<std::path::PathBuf> {
    let fallback_extension = extensions.first()?;
    if is_flatpak {
        return Some(path);
    }
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
pub(crate) fn choose_export_file_path(
    format: crate::pipeline::ExportFormat,
    default_name: &str,
    initial_directory: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    choose_save_path(
        tr_fmt("{format} image", &[("format", &format.label())]),
        format.extensions(),
        default_name,
        initial_directory,
    )
}

/// Save picker for the Edit Replay MP4.
pub(crate) fn choose_edit_replay_file_path(
    default_name: &str,
    initial_directory: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    choose_save_path(
        tr("MP4 video").to_owned(),
        MP4_EXTENSIONS,
        default_name,
        initial_directory,
    )
}

/// Extension list for [`choose_edit_replay_file_path`]; also the container FFmpeg is asked for.
const MP4_EXTENSIONS: &[&str] = &["mp4"];

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
        tr("More actions"),
        add_contents,
    )
}

pub(crate) fn mask_component_color(index: usize) -> eframe::egui::Color32 {
    use crate::ui::theme::MASK_COMPONENT_COLORS;
    MASK_COMPONENT_COLORS[index % MASK_COMPONENT_COLORS.len()]
}
