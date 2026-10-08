//! Application-level UI state: tabs, preferences, onboarding and platform chrome.

use super::*;

#[cfg(not(target_os = "android"))]
pub(crate) enum DesktopPickerEvent {
    RawFile(Option<PathBuf>),
    LibraryFolder(Option<PathBuf>),
    CameraProfileFolder(Option<PathBuf>),
    OnnxRuntime(Result<Option<(PathBuf, String)>, String>),
    PresetFiles(Option<Vec<PathBuf>>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AppTab {
    #[default]
    Library,
    Develop,
    Settings,
}

pub(crate) struct PreferencesState {
    pub(crate) image_relative_brush_size: bool,
    pub(crate) show_develop_navigation_labels: bool,
    pub(crate) automatic_lens_correction: AutomaticLensCorrection,
    pub(crate) export_name_template: String,
    #[cfg(not(target_os = "android"))]
    pub(crate) discord_rich_presence: bool,
    pub(crate) ui_design: UiDesign,
    pub(crate) preview_backdrop: PreviewBackdrop,
    pub(crate) onboarding_completed: bool,
    pub(crate) auto_check_updates: bool,
    pub(crate) github_update_check_allowed: Option<bool>,
    pub(crate) ignored_update_version: Option<String>,
    pub(crate) adjustment_copy_settings: AdjustmentCopySettings,
    pub(crate) performance_settings_path: Option<PathBuf>,
    pub(crate) camera_profile_mode: CameraProfileMode,
    pub(crate) camera_profile_folder: Option<PathBuf>,
    pub(crate) camera_profile_folder_label: Option<String>,
    pub(crate) camera_profile_auto_detect: bool,
    pub(crate) last_camera_profile: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum OnboardingStep {
    Appearance,
    Preview,
    CopyPaste,
    ExportNames,
    #[cfg(not(target_os = "android"))]
    Ai,
    #[cfg(not(target_os = "android"))]
    Discord,
}

#[derive(Clone, Debug)]
pub(crate) struct UnsupportedFileDialog {
    pub(crate) label: String,
    pub(crate) detail: String,
}

pub(crate) struct UiState {
    pub(crate) active_tab: AppTab,
    pub(crate) sidebar_tab: SidebarTab,
    pub(crate) status: String,
    pub(crate) adaptive_preview_backdrop: egui::Color32,
    pub(crate) notice: Option<String>,
    pub(crate) gpu_memory_error_dialog: bool,
    pub(crate) error_dialogs: ErrorDialogQueue,
    pub(crate) unsupported_file_dialog: Option<UnsupportedFileDialog>,
    pub(crate) onboarding_step: Option<OnboardingStep>,
    pub(in crate::app) version_check: version_update::VersionCheckState,
    pub(crate) thumbnail_cache_size: Option<Result<u64, String>>,
    pub(crate) thumbnail_cache_size_receiver: Option<mpsc::Receiver<Result<u64, String>>>,
    #[cfg(not(target_os = "android"))]
    pub(crate) desktop_picker_receiver: Option<mpsc::Receiver<DesktopPickerEvent>>,
}

#[cfg(target_os = "android")]
pub(crate) struct AndroidState {
    pub(crate) android_app: calibraw_ffi::AndroidApp,
    pub(crate) picker_pending: bool,
    pub(crate) pending_android_library_reset_reload: bool,
    pub(crate) camera_profile_folder_importing_label: Option<String>,
    pub(crate) pending_android_profile_reload: Option<ProfileReload>,
}
