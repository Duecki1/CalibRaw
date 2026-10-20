//! The window shown instead of the editor when the only graphics driver is one
//! CalibRaw cannot process photos on ([`calibraw_gpu::processing_backends`]).
//! The editor never starts there: its background shader compiles would trip
//! the GL backend's context-lock timeout and abort the program.

use super::CalibRawApp;
use crate::platform::Platform;
use eframe::{egui, wgpu};
use std::sync::Arc;

const TITLE: &str = "Unsupported graphics driver";

/// The app for a new window: the editor, or the unsupported-driver notice when
/// eframe could only start on an adapter outside the processing backends.
pub(crate) fn create_app(
    cc: &eframe::CreationContext<'_>,
    platform: Arc<dyn Platform>,
) -> Box<dyn eframe::App> {
    let unsupported = cc
        .wgpu_render_state
        .as_ref()
        .map(|render_state| render_state.adapter.get_info())
        .filter(|info| !calibraw_gpu::processing_backends().contains(info.backend.into()));
    match unsupported {
        Some(info) => Box::new(UnsupportedGpuApp::new(
            cc,
            platform.caps().is_desktop(),
            &info,
        )),
        None => Box::new(CalibRawApp::new(cc, platform)),
    }
}

struct UnsupportedGpuApp {
    message: String,
    /// Desktop windows offer Quit; Android apps are left with the system's
    /// navigation.
    can_quit: bool,
}

impl UnsupportedGpuApp {
    fn new(cc: &eframe::CreationContext<'_>, can_quit: bool, adapter: &wgpu::AdapterInfo) -> Self {
        crate::ui::theme::install(&cc.egui_ctx);
        let message = unsupported_driver_message(adapter);
        log::error!("{TITLE}: {message}");
        Self { message, can_quit }
    }
}

impl eframe::App for UnsupportedGpuApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        let ctx = ui.ctx().clone();
        let dialog = moduwu_design::Dialog::new("unsupported-gpu-dialog", TITLE);
        let body = |ui: &mut egui::Ui| {
            ui.label(&self.message);
        };
        if !self.can_quit {
            dialog.show(&ctx, body);
            return;
        }
        let mut quit = false;
        dialog.show_with_footer(&ctx, body, |ui| {
            quit |= moduwu_design::secondary_button(ui, "Quit").clicked();
            quit |= moduwu_design::dialog_keyboard_action(
                ui,
                moduwu_design::DialogKeyboard::CLOSE_ONLY,
                false,
            ) == moduwu_design::DialogAction::Cancel;
        });
        if quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn unsupported_driver_message(adapter: &wgpu::AdapterInfo) -> String {
    let offered = match adapter.backend {
        wgpu::Backend::Gl => "OpenGL",
        other => other.to_str(),
    };
    let mut message = format!(
        "CalibRaw needs a {} graphics driver. The only driver found is {offered} on {}, which CalibRaw cannot process photos with.",
        crate::platform::PROCESSING_GRAPHICS_API,
        adapter.name
    );
    if let Some(hint) = crate::platform::PROCESSING_DRIVER_INSTALL_HINT {
        message.push(' ');
        message.push_str(hint);
    }
    message
}
