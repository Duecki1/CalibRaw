//! Platform differences of the UI, decided once.
//!
//! - [`PlatformCaps`]: what the device offers. Views and handlers branch on
//!   these values, so every branch compiles, and can be tested, on every host.
//! - [`Platform`]: the operating-system services behind actions (the Android
//!   bridge, its pickers, media library, publishing and notifications).
//!   Handlers reach it through `CalibRawApp::platform`; tests use
//!   `fake::FakePlatform`.
//! - Desktop services without state: native file dialogs, the trash, the
//!   Flatpak portal and per-OS folders.
//!
//! This is the only module of `calibraw-ui` that names a `target_os`
//! (`cargo xtask arch-check` enforces it).

// Each target runs one backend, which builds only its own capabilities.
#[allow(dead_code, reason = "the other backend's capabilities are unused")]
mod caps;
pub(crate) mod discord;
mod files;
mod fonts;
mod graphics;
mod service;

#[cfg(target_os = "android")]
mod android;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop;
#[cfg(test)]
pub(crate) mod fake;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "ios")]
mod ios_library;

pub(crate) use caps::{MemoryClass, PlatformCaps, StorageKind, VideoEncoder};
pub(crate) use files::{
    camera_profile_search_locations, desktop_settings_path, detected_camera_profile_folder,
    is_flatpak, is_onnx_runtime_library_name, show_fatal_message, trash, trash_name, FileDialog,
    ONNX_RUNTIME_IS_DLL, PRIMARY_MODIFIER_NAME,
};
pub(crate) use fonts::{chinese_font, SystemFont};
pub(crate) use graphics::{PROCESSING_DRIVER_INSTALL_HINT, PROCESSING_GRAPHICS_API};
pub(crate) use service::{
    CameraProfileFolderResult, ExportPublishResult, ImportProgress, ImportedLibraryDocument,
    LibraryDocument, LibraryFolder, PickerResult, Platform, PublishedExport, ReplayVideoEncoder,
    StagedFilesResult, TaskNotification,
};

#[cfg(target_os = "android")]
pub(crate) use android::main;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) use desktop::main;
#[cfg(target_os = "ios")]
pub(crate) use ios::main;
