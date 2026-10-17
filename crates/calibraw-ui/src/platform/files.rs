//! Desktop file services: native file dialogs, the trash, the Flatpak portal
//! and per-OS folders. Where a platform lacks one (Android), dialogs return
//! nothing and the rest reports that it is unavailable; callers check
//! [`super::PlatformCaps`] first.

use std::path::{Path, PathBuf};

/// A native open, save or folder dialog.
#[derive(Clone, Debug, Default)]
pub(crate) struct FileDialog {
    pub(in crate::platform) title: Option<String>,
    pub(in crate::platform) directory: Option<PathBuf>,
    pub(in crate::platform) file_name: Option<String>,
    pub(in crate::platform) filters: Vec<(String, Vec<String>)>,
}

impl FileDialog {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub(crate) fn set_directory(mut self, directory: impl AsRef<Path>) -> Self {
        self.directory = Some(directory.as_ref().to_path_buf());
        self
    }

    pub(crate) fn set_file_name(mut self, file_name: impl Into<String>) -> Self {
        self.file_name = Some(file_name.into());
        self
    }

    pub(crate) fn add_filter(
        mut self,
        name: impl Into<String>,
        extensions: &[impl ToString],
    ) -> Self {
        self.filters.push((
            name.into(),
            extensions.iter().map(ToString::to_string).collect(),
        ));
        self
    }

    /// Asks for one file and waits for the answer. Runs the dialog
    /// asynchronously, so call it on a worker thread.
    pub(crate) fn pick_file_on_worker(self) -> Option<PathBuf> {
        imp::pick_file_on_worker(self)
    }

    /// Asks for any number of files; see [`Self::pick_file_on_worker`].
    pub(crate) fn pick_files_on_worker(self) -> Option<Vec<PathBuf>> {
        imp::pick_files_on_worker(self)
    }

    /// Asks for a folder; see [`Self::pick_file_on_worker`].
    pub(crate) fn pick_folder_on_worker(self) -> Option<PathBuf> {
        imp::pick_folder_on_worker(self)
    }

    /// Asks for a folder in a modal dialog on the calling (UI) thread.
    pub(crate) fn pick_folder(self) -> Option<PathBuf> {
        imp::pick_folder(self)
    }

    /// Asks where to save, in a modal dialog on the calling (UI) thread.
    pub(crate) fn save_file(self) -> Option<PathBuf> {
        imp::save_file(self)
    }
}

/// Moves a file to the trash: through the Flatpak portal inside Flatpak,
/// otherwise through the desktop's own trash.
pub(crate) fn trash(path: &Path) -> Result<(), String> {
    imp::trash(path)
}

/// The primary shortcut modifier in the platform's own spelling (egui's
/// `COMMAND`).
pub(crate) const PRIMARY_MODIFIER_NAME: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Ctrl"
};

/// Whether a manually chosen ONNX Runtime is a Windows `onnxruntime.dll`.
pub(crate) const ONNX_RUNTIME_IS_DLL: bool = cfg!(target_os = "windows");

/// What the desktop calls its trash, for messages.
pub(crate) const fn trash_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "Recycle Bin"
    } else {
        "Trash"
    }
}

/// Whether CalibRaw runs inside Flatpak, whose document grants and host
/// services need portal-specific handling.
pub(crate) fn is_flatpak() -> bool {
    imp::is_flatpak()
}

/// A native message box for failures the UI can no longer draw, such as a
/// lost GPU device. Blocks until dismissed.
pub(crate) fn show_fatal_message(title: &str, description: &str) {
    imp::show_fatal_message(title, description);
}

/// Whether `file_name` (lowercase) names this OS's ONNX Runtime shared library.
pub(crate) fn is_onnx_runtime_library_name(file_name: &str) -> bool {
    if cfg!(target_os = "windows") {
        file_name == "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        file_name == "libonnxruntime.dylib"
            || (file_name.starts_with("libonnxruntime.") && file_name.ends_with(".dylib"))
    } else {
        file_name == "libonnxruntime.so" || file_name.starts_with("libonnxruntime.so.")
    }
}

/// Folders searched for DCP camera profiles, as shown during setup. Empty
/// where auto-detect has no standard locations.
pub(crate) fn camera_profile_search_locations() -> &'static [&'static str] {
    imp::CAMERA_PROFILE_SEARCH_LOCATIONS
}

/// The desktop settings file, for workers that read it without the app.
/// `None` where the platform hands out its location at run time (Android).
pub(crate) fn desktop_settings_path() -> Option<PathBuf> {
    imp::performance_settings_path()
}

/// The first standard DCP camera-profile folder that exists.
pub(crate) fn detected_camera_profile_folder() -> Option<PathBuf> {
    imp::camera_profile_candidates()
        .into_iter()
        .find(|path| path.is_dir())
}

#[cfg(not(target_os = "android"))]
use super::desktop::files as imp;

#[cfg(target_os = "android")]
mod imp {
    use super::FileDialog;
    use std::path::{Path, PathBuf};

    pub(super) const CAMERA_PROFILE_SEARCH_LOCATIONS: &[&str] = &[];

    pub(super) fn pick_file_on_worker(_dialog: FileDialog) -> Option<PathBuf> {
        None
    }

    pub(super) fn pick_files_on_worker(_dialog: FileDialog) -> Option<Vec<PathBuf>> {
        None
    }

    pub(super) fn pick_folder_on_worker(_dialog: FileDialog) -> Option<PathBuf> {
        None
    }

    pub(super) fn pick_folder(_dialog: FileDialog) -> Option<PathBuf> {
        None
    }

    pub(super) fn save_file(_dialog: FileDialog) -> Option<PathBuf> {
        None
    }

    pub(super) fn trash(_path: &Path) -> Result<(), String> {
        Err("The trash is not available on Android".to_owned())
    }

    pub(super) fn is_flatpak() -> bool {
        false
    }

    pub(super) fn show_fatal_message(title: &str, description: &str) {
        log::error!("{title}: {description}");
    }

    pub(super) fn camera_profile_candidates() -> Vec<PathBuf> {
        Vec::new()
    }

    pub(super) fn performance_settings_path() -> Option<PathBuf> {
        None
    }
}
