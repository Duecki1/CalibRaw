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

/// Desktop-only “save as” picker. Flatpak filenames must match the exact portal
/// grant; the export format is passed separately to the encoder.
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
    extensions.first()?;
    save_path_with_extension(
        dialog.save_file()?,
        extensions,
        crate::desktop_portal::is_flatpak(),
    )
}

/// Apply native extension defaults without changing Flatpak document grants.
#[cfg(not(target_os = "android"))]
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

#[cfg(all(test, not(target_os = "android")))]
mod save_path_tests {
    use super::save_path_with_extension;
    use std::path::PathBuf;

    #[test]
    fn flatpak_preserves_exact_granted_filename_for_every_extension() {
        for name in [
            "export",
            "export.png",
            "export.JPEG",
            "export.",
            "my.photo.txt",
        ] {
            let granted = PathBuf::from("/run/user/1000/doc/grant").join(name);
            assert_eq!(
                save_path_with_extension(granted.clone(), &["jpg", "jpeg"], true),
                Some(granted)
            );
        }
        let granted = PathBuf::from("/run/user/1000/doc/grant/replay.wrong");
        assert_eq!(
            save_path_with_extension(granted.clone(), super::MP4_EXTENSIONS, true),
            Some(granted)
        );
    }

    #[test]
    fn native_corrects_missing_or_wrong_extensions_and_preserves_valid_aliases() {
        for (name, expected) in [
            ("export", "export.jpg"),
            ("export.png", "export.jpg"),
            ("export.", "export.jpg"),
            ("my.photo.txt", "my.photo.jpg"),
            ("export.jpg", "export.jpg"),
            ("export.JPEG", "export.JPEG"),
        ] {
            assert_eq!(
                save_path_with_extension(PathBuf::from(name), &["jpg", "jpeg"], false),
                Some(PathBuf::from(expected))
            );
        }
        assert_eq!(
            save_path_with_extension(PathBuf::from("replay.wrong"), super::MP4_EXTENSIONS, false),
            Some(PathBuf::from("replay.mp4"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn flatpak_preserves_non_utf8_filenames_while_native_replaces_invalid_extension() {
        use std::os::unix::ffi::OsStringExt;
        let granted = PathBuf::from(std::ffi::OsString::from_vec(b"export.\xff".to_vec()));
        assert_eq!(
            save_path_with_extension(granted.clone(), &["jpg"], true),
            Some(granted.clone())
        );
        assert_eq!(
            save_path_with_extension(granted, &["jpg"], false),
            Some(PathBuf::from("export.jpg"))
        );
    }
}

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
