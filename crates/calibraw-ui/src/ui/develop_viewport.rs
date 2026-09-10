use crate::app::{CalibRawApp, SidebarTab};
use crate::ui::{layout::ScreenLayout, preview::Preview, sidebar::Sidebar, top_bar::TopBar};
use eframe::egui::{self, Rect, Sense, Ui};

const HANDLE_HEIGHT: f32 = 24.0;
const COLLAPSED_HEIGHT: f32 = 144.0;

fn sheet_height(viewport_height: f32, requested: f32) -> f32 {
    let maximum = (viewport_height * 0.65).max(1.0);
    requested.clamp(COLLAPSED_HEIGHT.min(maximum), maximum)
}

/// The canvas owns the entire safe area. Tool surfaces never participate in
/// its width/scale calculation, and gestures only start in the exposed image.
pub(crate) fn show(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
    #[cfg(target_os = "android")]
    if ui.available_width() >= ui.available_height() {
        show_landscape(ui, app, frame);
        return;
    }
    let canvas = ui.available_rect_before_wrap();
    let context = ui.ctx().clone();
    let top_height = crate::ui::theme::TOOLBAR_HEIGHT + 12.0;
    let top = Rect::from_min_size(canvas.min, egui::vec2(canvas.width(), top_height));
    let height_id = egui::Id::new("portrait-tool-sheet-height");
    let default_height = if app.ui.sidebar_tab == SidebarTab::Masks {
        (canvas.height() * 0.45).max(360.0)
    } else {
        (canvas.height() * 0.34).max(260.0)
    };
    let mut requested = context
        .data(|data| data.get_temp::<f32>(height_id))
        .unwrap_or(default_height);
    let tab_id = height_id.with("tab");
    let previous_tab = context.data(|data| data.get_temp::<SidebarTab>(tab_id));
    if app.ui.sidebar_tab == SidebarTab::Masks && previous_tab != Some(SidebarTab::Masks) {
        requested = requested.max(default_height);
        context.data_mut(|data| data.insert_temp(height_id, requested));
    }
    context.data_mut(|data| data.insert_temp(tab_id, app.ui.sidebar_tab));
    let mut height = sheet_height(canvas.height(), requested);

    show_top_controls(&context, top, app, frame);

    let sheet = Rect::from_min_max(
        egui::pos2(canvas.left(), canvas.bottom() - height),
        canvas.max,
    );
    egui::Area::new(egui::Id::new("portrait-tool-sheet"))
        .order(egui::Order::Middle)
        .fixed_pos(sheet.min)
        .movable(false)
        .constrain(false)
        .show(&context, |ui| {
            ui.set_min_size(sheet.size());
            ui.set_max_size(sheet.size());
            ui.set_clip_rect(sheet);
            ui.painter()
                .rect_filled(sheet, 0.0, ui.visuals().panel_fill.gamma_multiply(0.97));
            let handle = Rect::from_min_size(sheet.min, egui::vec2(sheet.width(), HANDLE_HEIGHT));
            let response = ui
                .interact(handle, height_id, Sense::click_and_drag())
                .on_hover_cursor(egui::CursorIcon::ResizeVertical)
                .on_hover_text("Drag to resize tools; tap to collapse or expand");
            ui.painter().rect_filled(
                Rect::from_center_size(handle.center(), egui::vec2(36.0, 4.0)),
                2.0,
                ui.visuals().weak_text_color(),
            );
            if response.dragged() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    height = sheet_height(
                        canvas.height(),
                        canvas.bottom() - pointer.y + HANDLE_HEIGHT * 0.5,
                    );
                    context.data_mut(|data| data.insert_temp(height_id, height));
                }
            } else if response.clicked() {
                height = sheet_height(
                    canvas.height(),
                    if height <= COLLAPSED_HEIGHT + 1.0 {
                        default_height
                    } else {
                        COLLAPSED_HEIGHT
                    },
                );
                context.data_mut(|data| data.insert_temp(height_id, height));
            }
            let content = Rect::from_min_max(egui::pos2(sheet.left(), handle.bottom()), sheet.max);
            let mut content_ui = ui.new_child(egui::UiBuilder::new().max_rect(content));
            content_ui.set_clip_rect(content);
            if app.ui.sidebar_tab == SidebarTab::Masks && height > COLLAPSED_HEIGHT + 100.0 {
                egui::Panel::top("portrait-mask-strip")
                    .resizable(false)
                    .exact_size(Sidebar::VERTICAL_MASK_STRIP_HEIGHT)
                    .frame(egui::Frame::NONE)
                    .show(&mut content_ui, |ui| {
                        Sidebar::show_vertical_mask_strip(ui, app, frame)
                    });
            }
            Sidebar::show(&mut content_ui, app, ScreenLayout::Vertical, frame);
        });

    let exposed = Rect::from_min_max(
        egui::pos2(canvas.left(), top.bottom().min(sheet.top())),
        egui::pos2(canvas.right(), sheet.top()),
    );
    Preview::show_in_viewport(ui, app, frame, Some(exposed));
}

fn show_top_controls(
    context: &egui::Context,
    top: Rect,
    app: &mut CalibRawApp,
    frame: &eframe::Frame,
) {
    egui::Area::new(egui::Id::new("portrait-top-controls"))
        .order(egui::Order::Middle)
        .fixed_pos(top.min)
        .movable(false)
        .constrain(false)
        .show(context, |ui| {
            ui.set_min_size(top.size());
            ui.set_max_size(top.size());
            ui.set_clip_rect(top);
            egui::Frame::new()
                .fill(ui.visuals().panel_fill.gamma_multiply(0.92))
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| TopBar::show_portrait(ui, app, frame));
        });
}

#[cfg(target_os = "android")]
fn show_landscape(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
    let canvas = ui.available_rect_before_wrap();
    let context = ui.ctx().clone();
    let top = Rect::from_min_size(
        canvas.min,
        egui::vec2(canvas.width(), crate::ui::theme::TOOLBAR_HEIGHT + 12.0),
    );
    show_top_controls(&context, top, app, frame);
    let rail_width = Sidebar::ANDROID_LANDSCAPE_TOOL_RAIL_WIDTH;
    let rail = Rect::from_min_max(
        egui::pos2(canvas.right() - rail_width, top.bottom()),
        canvas.max,
    );
    let tools_width = ScreenLayout::Horizontal
        .sidebar_default_size(canvas.size())
        .min(canvas.width() * 0.48);
    let tools = Rect::from_min_max(
        egui::pos2(rail.left() - tools_width, top.bottom()),
        rail.left_bottom(),
    );
    egui::Area::new(egui::Id::new("landscape-overlay-tool-rail"))
        .order(egui::Order::Middle)
        .fixed_pos(rail.min)
        .movable(false)
        .constrain(false)
        .show(&context, |ui| {
            ui.set_min_size(rail.size());
            ui.set_max_size(rail.size());
            ui.set_clip_rect(rail);
            ui.painter().rect_filled(rail, 0.0, ui.visuals().panel_fill);
            Sidebar::show_android_landscape_primary_tabs(ui, app);
        });
    egui::Area::new(egui::Id::new("landscape-overlay-tools"))
        .order(egui::Order::Middle)
        .fixed_pos(tools.min)
        .movable(false)
        .constrain(false)
        .show(&context, |ui| {
            ui.set_min_size(tools.size());
            ui.set_max_size(tools.size());
            ui.set_clip_rect(tools);
            ui.painter()
                .rect_filled(tools, 0.0, ui.visuals().panel_fill.gamma_multiply(0.97));
            let mut content = ui.new_child(egui::UiBuilder::new().max_rect(tools.shrink(8.0)));
            if app.ui.sidebar_tab == SidebarTab::Masks {
                egui::Panel::top("landscape-overlay-masks")
                    .resizable(false)
                    .exact_size(Sidebar::VERTICAL_MASK_STRIP_HEIGHT)
                    .frame(egui::Frame::NONE)
                    .show(&mut content, |ui| {
                        Sidebar::show_vertical_mask_strip(ui, app, frame)
                    });
            }
            Sidebar::show(&mut content, app, ScreenLayout::Horizontal, frame);
        });
    let exposed = Rect::from_min_max(egui::pos2(canvas.left(), top.bottom()), tools.left_bottom());
    Preview::show_in_viewport(ui, app, frame, Some(exposed));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_resizing_always_leaves_image_room() {
        for viewport in [200.0, 640.0, 891.0, 1400.0] {
            for requested in [-100.0, 144.0, 380.0, 2000.0] {
                let height = sheet_height(viewport, requested);
                assert!(height > 0.0 && height <= viewport * 0.65);
            }
        }
    }
}
