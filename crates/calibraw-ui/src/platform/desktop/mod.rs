//! The desktop backend (Linux, Windows, macOS).

pub(super) mod discord;
mod editors;
mod entry;
pub(super) mod files;
#[cfg(target_os = "macos")]
mod macos_menu;
#[cfg(target_os = "windows")]
mod nvidia_vrr;
mod portal;
#[cfg(target_os = "linux")]
mod wayland_pinch;

pub(crate) use entry::main;

use super::service::{GpuLossRecovery, Platform};
use super::PlatformCaps;
use std::path::PathBuf;
use std::sync::Arc;

/// Set in a process started by GPU-loss recovery, so a GPU
/// that fails again right away does not restart CalibRaw in a loop.
const RESTARTED_AFTER_GPU_LOSS: &str = "CALIBRAW_RESTARTED_AFTER_GPU_LOSS";

pub(crate) struct DesktopPlatform {
    /// The touchpad pinch winit does not forward on Wayland; `None` once
    /// the UI detaches, and on X11.
    #[cfg(target_os = "linux")]
    wayland_pinch: std::sync::Mutex<Option<wayland_pinch::WaylandPinch>>,
}

impl DesktopPlatform {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Arc<Self> {
        #[cfg(not(target_os = "linux"))]
        let _ = cc;
        Arc::new(Self {
            #[cfg(target_os = "linux")]
            wayland_pinch: std::sync::Mutex::new(wayland_pinch::WaylandPinch::start(cc)),
        })
    }
}

impl Platform for DesktopPlatform {
    fn caps(&self) -> PlatformCaps {
        PlatformCaps::DESKTOP
    }

    #[cfg(target_os = "macos")]
    fn attach_ui(&self, request_repaint: Box<dyn Fn() + Send + Sync>) {
        macos_menu::set_wake_ui(Some(request_repaint));
    }

    fn detach_ui(&self) {
        #[cfg(target_os = "macos")]
        macos_menu::set_wake_ui(None);
        #[cfg(target_os = "linux")]
        drop(
            self.wayland_pinch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take(),
        );
    }

    #[cfg(target_os = "linux")]
    fn take_input_events(&self, events: &mut Vec<eframe::egui::Event>) {
        if let Some(pinch) = self
            .wayland_pinch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            pinch.take_events(events);
        }
    }

    #[cfg(target_os = "macos")]
    fn take_settings_request(&self) -> bool {
        macos_menu::take_settings_request()
    }

    fn performance_settings_path(&self) -> Result<Option<PathBuf>, String> {
        Ok(performance_settings_path())
    }

    fn model_root(&self) -> PathBuf {
        calibraw_ai::desktop_model_root()
    }

    fn reveal_file(&self, path: &std::path::Path) -> Result<(), String> {
        files::reveal_file(path)
    }

    fn edit_in_support(&self) -> super::EditInSupport {
        editors::support()
    }

    fn installed_editors(&self) -> Vec<crate::edit_in::ExternalEditor> {
        editors::installed()
    }

    fn editor_program_dialog(&self) -> super::FileDialog {
        editors::program_dialog()
    }

    fn editor_for_program(
        &self,
        program: &std::path::Path,
    ) -> Option<crate::edit_in::ExternalEditor> {
        editors::editor_for_program(program)
    }

    fn open_in_editor(
        &self,
        program: Option<&std::path::Path>,
        file: &std::path::Path,
    ) -> Result<(), String> {
        editors::open(program, file)
    }

    fn edit_in_cache_dir(&self) -> Option<PathBuf> {
        Some(calibraw_core::thumbnail_cache::desktop_app_cache_root().join("edit-in"))
    }

    fn gpu_pipeline_cache_dir(&self) -> Result<Option<PathBuf>, String> {
        Ok(Some(
            calibraw_core::thumbnail_cache::desktop_app_cache_root(),
        ))
    }

    fn gpu_ai_job_record_path(&self) -> Option<PathBuf> {
        Some(calibraw_core::thumbnail_cache::desktop_app_cache_root().join("gpu-ai-job.txt"))
    }

    /// Ends the lost render process after its notice; restarts it once with
    /// the same arguments, outside Flatpak, whose sandbox ends with the process.
    fn gpu_loss_recovery(&self) -> GpuLossRecovery {
        // An AppImage runs from a temporary mount; restart the image itself.
        let program = std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok());
        let restart = program.filter(|_| {
            std::env::var_os(RESTARTED_AFTER_GPU_LOSS).is_none()
                && std::env::var_os("FLATPAK_ID").is_none()
        });
        GpuLossRecovery::new(restart.is_some(), move |title, message| {
            files::show_fatal_message(title, message);
            if let Some(program) = restart {
                if let Err(error) = std::process::Command::new(program)
                    .args(std::env::args_os().skip(1))
                    .env(RESTARTED_AFTER_GPU_LOSS, "1")
                    .spawn()
                {
                    log::error!("could not restart CalibRaw after GPU loss: {error}");
                }
            }
            // State is already saved. Do not unwind through a lost painter
            // or swapchain: their teardown can panic a second time.
            entry::end_process(Ok(()))
        })
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
