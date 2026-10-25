//! Application-level UI state: tabs, preferences, onboarding and platform chrome.

use super::*;

pub(crate) enum DesktopPickerEvent {
    RawFile(Option<PathBuf>),
    LibraryFolder(Option<PathBuf>),
    CameraProfileFolder(Option<PathBuf>),
    OnnxRuntime(Result<Option<(PathBuf, calibraw_ai::Sha256)>, String>),
    PresetFiles(Option<Vec<PathBuf>>),
    LutFiles(Option<Vec<PathBuf>>),
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
    pub(crate) develop_card_order: DevelopCardOrder,
    pub(crate) automatic_lens_correction: AutomaticLensCorrection,
    /// Ask about manual correction when automatic correction finds no profile.
    pub(crate) missing_lens_profile_prompt: bool,
    pub(crate) new_photo_preset: Option<PathBuf>,
    pub(crate) export_name_template: String,
    pub(crate) discord_rich_presence: bool,
    pub(crate) hdd_mode: bool,
    pub(crate) ui_design: UiDesign,
    pub(crate) preview_backdrop: PreviewBackdrop,
    pub(crate) onboarding_completed: bool,
    pub(crate) subject_mask_quality_chosen: bool,
    pub(crate) tips: TipPreferences,
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
    SubjectMasks,
    CopyPaste,
    ExportNames,
    /// Desktop only: Android imports profiles from Settings.
    CameraProfiles,
    /// Desktop only, where Discord can be reached.
    Discord,
}

/// A Settings dialog for a list too long for its card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsDialog {
    DevelopCardOrder,
    Luts,
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
    pub(crate) error_dialogs: ErrorDialogQueue,
    pub(crate) unsupported_file_dialog: Option<UnsupportedFileDialog>,
    /// The message of the open missing-lens-profile prompt.
    pub(crate) missing_lens_profile_prompt: Option<String>,
    /// The message of the open restart prompt, shown after an ONNX Runtime
    /// choice changed while another runtime is already loaded.
    pub(crate) runtime_restart_prompt: Option<String>,
    pub(crate) onboarding_step: Option<OnboardingStep>,
    /// The feature tip offered and not yet answered.
    pub(in crate::app) tip: Option<ShownTip>,
    pub(in crate::app) version_check: version_update::VersionCheckState,
    /// The keyboard shortcuts reference is open.
    pub(crate) shortcuts_dialog_open: bool,
    /// The Settings dialog that is open, shown while Settings is.
    pub(crate) settings_dialog: Option<SettingsDialog>,
    pub(crate) thumbnail_cache_size: Option<Result<u64, String>>,
    pub(crate) thumbnail_cache_size_receiver: Option<mpsc::Receiver<Result<u64, String>>>,
    pub(crate) desktop_picker_receiver: Option<mpsc::Receiver<DesktopPickerEvent>>,
    /// The platform was last asked for the soft keyboard's number layout.
    pub(in crate::app) numeric_keyboard: bool,
}

/// Media-library workflows the platform finishes asynchronously: its picker,
/// library reloads and imports. Idle where storage is folders.
pub(crate) struct AndroidState {
    pub(crate) picker_pending: bool,
    pub(crate) pending_android_library_reset_reload: bool,
    pub(crate) camera_profile_folder_importing_label: Option<String>,
    pub(crate) pending_android_profile_reload: Option<ProfileReload>,
    /// A multi-photo import the library shows progress for, until the
    /// imported photos appear.
    pub(crate) photo_import: Option<PhotoImportStatus>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhotoImportStatus {
    /// The platform is copying the picked photos into the library.
    Copying(crate::platform::ImportProgress),
    /// The copy finished; the library is rescanning to show the photos.
    Loading,
}
