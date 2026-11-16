//! The portrait layout's context bar: the current tab's selector, at the top
//! of the tool sheet above its scrolling controls, so it stays in reach when
//! the sheet collapses.

use super::*;

/// The selector a tab moves into the context bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextBar {
    /// The mask group and sub-mask cards.
    MaskStrip,
    /// The crop's aspect-ratio chips and orientation.
    CropAspect,
    /// The retouch tools.
    RetouchTools,
}

impl ContextBar {
    /// The bar the current tab shows, if any.
    pub(crate) fn of(app: &CalibRawApp) -> Option<Self> {
        match app.ui.sidebar_tab {
            SidebarTab::Masks if Sidebar::shows_mask_strip(app) => Some(Self::MaskStrip),
            SidebarTab::Crop => Some(Self::CropAspect),
            SidebarTab::Inpainting => Some(Self::RetouchTools),
            _ => None,
        }
    }

    /// The bar's height in points, including its margins.
    pub(crate) fn height(self) -> f32 {
        match self {
            Self::MaskStrip => Sidebar::VERTICAL_MASK_STRIP_HEIGHT,
            Self::CropAspect | Self::RetouchTools => {
                moduwu_design::CONTROL_HEIGHT + 2.0 * moduwu_design::SPACE_SM
            }
        }
    }
}

impl Sidebar {
    /// The contents of the context `bar`, laid out across `ui`.
    pub(crate) fn show_context_bar(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        frame: &eframe::Frame,
        bar: ContextBar,
    ) {
        match bar {
            ContextBar::MaskStrip => Self::show_vertical_mask_strip(ui, app, frame),
            ContextBar::CropAspect => {
                Self::padded_bar_row(ui, |ui| Self::show_crop_aspect_bar(ui, app));
            }
            ContextBar::RetouchTools => {
                Self::padded_bar_row(ui, |ui| Self::show_inpaint_tool_row(ui, app));
            }
        }
    }

    /// A single row of controls, inset like the sheet's cards and centred
    /// in the bar's height.
    fn padded_bar_row(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(
                moduwu_design::CONTENT_MARGIN,
                moduwu_design::SPACE_SM as i8,
            ))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                contents(ui);
            });
    }
}
