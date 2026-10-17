//! The desktop backend (Linux, Windows, macOS).

pub(super) mod discord;
mod entry;
pub(super) mod files;
mod portal;

pub(crate) use entry::main;

use super::service::Platform;
use super::PlatformCaps;
use std::path::PathBuf;
use std::sync::Arc;

/// Set in a process started by `restart_after_gpu_loss_command`, so a GPU
/// that fails again right away does not restart CalibRaw in a loop.
const RESTARTED_AFTER_GPU_LOSS: &str = "CALIBRAW_RESTARTED_AFTER_GPU_LOSS";

pub(crate) struct DesktopPlatform;

impl DesktopPlatform {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl Platform for DesktopPlatform {
    fn caps(&self) -> PlatformCaps {
        PlatformCaps::DESKTOP
    }

    fn performance_settings_path(&self) -> Result<Option<PathBuf>, String> {
        Ok(performance_settings_path())
    }

    fn model_root(&self) -> PathBuf {
        calibraw_ai::desktop_model_cache_root()
    }

    fn gpu_pipeline_cache_dir(&self) -> Result<Option<PathBuf>, String> {
        Ok(Some(
            calibraw_core::thumbnail_cache::desktop_app_cache_root(),
        ))
    }

    fn gpu_ai_job_record_path(&self) -> Option<PathBuf> {
        Some(calibraw_core::thumbnail_cache::desktop_app_cache_root().join("gpu-ai-job.txt"))
    }

    /// This program with its arguments, or `None` after a restart that
    /// already followed a GPU loss, and inside Flatpak, whose sandbox ends
    /// with this process.
    fn restart_after_gpu_loss_command(&self) -> Option<std::process::Command> {
        if std::env::var_os(RESTARTED_AFTER_GPU_LOSS).is_some()
            || std::env::var_os("FLATPAK_ID").is_some()
        {
            return None;
        }
        // An AppImage runs from a temporary mount; restart the image itself.
        let program = match std::env::var_os("APPIMAGE") {
            Some(image) => PathBuf::from(image),
            None => std::env::current_exe().ok()?,
        };
        let mut command = std::process::Command::new(program);
        command
            .args(std::env::args_os().skip(1))
            .env(RESTARTED_AFTER_GPU_LOSS, "1");
        Some(command)
    }

    fn clear_thumbnail_cache(&self) -> Result<(), String> {
        calibraw_core::thumbnail_cache::clear_desktop_thumbnail_cache()
    }

    fn thumbnail_cache_size_bytes(&self) -> Result<u64, String> {
        calibraw_core::thumbnail_cache::desktop_thumbnail_cache_size_bytes()
    }
}

pub(in crate::platform) fn performance_settings_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);

    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support"));

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".config"))
        });

    #[cfg(not(any(windows, unix, target_os = "macos")))]
    let base: Option<PathBuf> = None;

    base.map(|base| base.join("calibraw").join("performance.json"))
}
