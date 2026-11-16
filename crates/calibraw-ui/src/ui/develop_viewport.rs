use crate::app::CalibRawApp;
use crate::ui::sidebar::{ContextBar, Sidebar};
use crate::ui::top_bar::TopBar;
use eframe::egui::{self, Rect, Sense, Ui};
use moduwu_design::ScreenLayout;

const HANDLE_HEIGHT: f32 = 24.0;
const COLLAPSED_HEIGHT: f32 = 144.0;
/// The photo height an expanded sheet always leaves below the top controls.
const MIN_PHOTO_HEIGHT: f32 = 160.0;

/// The height of the sheet's handle and contents for a `requested` one, in a
/// viewport of `viewport_height` of which `reserved` points are taken by the
/// top controls and the context bar.
fn sheet_height(viewport_height: f32, reserved: f32, requested: f32) -> f32 {
    let maximum = (viewport_height * 0.65)
        .min(viewport_height - reserved - MIN_PHOTO_HEIGHT)
        .max(1.0);
    requested.clamp(COLLAPSED_HEIGHT.min(maximum), maximum)
}

/// The canvas owns the entire safe area. Tool surfaces never participate in
/// its width/scale calculation, and gestures only start in the exposed image.
///
/// The tool sheet stacks, from its top: the resize handle, the current tab's
/// context bar ([`ContextBar`]), and the tab's controls. The context bar
/// stays when the sheet collapses, so the tab's selector is always at hand.
pub(crate) fn show(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
    // A phone held sideways puts the tools in a rail beside the photo.
    if app.caps().is_mobile() && ui.available_width() >= ui.available_height() {
        show_landscape(ui, app, frame);
        return;
    }
    let canvas = ui.available_rect_before_wrap();
    let context = ui.ctx().clone();
    let top_height = moduwu_design::TOOLBAR_HEIGHT + 12.0;
    let top = Rect::from_min_size(canvas.min, egui::vec2(canvas.width(), top_height));
    let bar = ContextBar::of(app);
    let bar_height = bar.map_or(0.0, ContextBar::height);
    let reserved = top_height + bar_height;
    let height_id = egui::Id::new("portrait-tool-sheet-height");
    let default_height = (canvas.height() * 0.45).max(360.0);
    // The stored height is the user's expanded size. A closed sidebar collapses
    // the sheet to its tab bar without forgetting that size.
    let requested = context
        .data(|data| data.get_temp::<f32>(height_id))
        .filter(|height| *height > COLLAPSED_HEIGHT + 1.0)
        .unwrap_or(default_height);
    let mut height = if app.develop_ui.sidebar_open {
        sheet_height(canvas.height(), reserved, requested)
    } else {
        sheet_height(canvas.height(), reserved, COLLAPSED_HEIGHT)
    };

    show_top_controls(&context, top, app, frame);

    let sheet = Rect::from_min_max(
        egui::pos2(canvas.left(), canvas.bottom() - height - bar_height),
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
                .on_hover_cursor(egui::CursorIcon::ResizeVertical);
            ui.painter().rect_filled(
                Rect::from_center_size(handle.center(), egui::vec2(36.0, 4.0)),
                2.0,
                ui.visuals().weak_text_color(),
            );
            if response.dragged() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    height = sheet_height(
                        canvas.height(),
                        reserved,
                        canvas.bottom() - pointer.y + HANDLE_HEIGHT * 0.5 - bar_height,
                    );
                    app.develop_ui.sidebar_open = height > COLLAPSED_HEIGHT + 1.0;
                    context.data_mut(|data| data.insert_temp(height_id, height));
                }
            } else if response.clicked() {
                app.develop_ui.sidebar_open = !app.develop_ui.sidebar_open;
                height = if app.develop_ui.sidebar_open {
                    sheet_height(canvas.height(), reserved, requested)
                } else {
                    sheet_height(canvas.height(), reserved, COLLAPSED_HEIGHT)
                };
            }
            let mut content_top = handle.bottom();
            if let Some(bar) = bar {
                let rect = Rect::from_min_size(
                    egui::pos2(sheet.left(), content_top),
                    egui::vec2(sheet.width(), bar_height),
                );
                let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                bar_ui.set_clip_rect(rect);
                Sidebar::show_context_bar(&mut bar_ui, app, frame, bar);
                let stroke =
                    egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
                ui.painter().hline(rect.x_range(), rect.bottom(), stroke);
                content_top = rect.bottom();
            }
            let content = Rect::from_min_max(egui::pos2(sheet.left(), content_top), sheet.max);
            let mut content_ui = ui.new_child(egui::UiBuilder::new().max_rect(content));
            content_ui.set_clip_rect(content);
            Sidebar::show(&mut content_ui, app, ScreenLayout::Vertical, frame);
        });

    let exposed = Rect::from_min_max(
        egui::pos2(canvas.left(), top.bottom().min(sheet.top())),
        egui::pos2(canvas.right(), sheet.top()),
    );
    app.show_preview(ui, frame, Some(exposed));
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
                .inner_margin(egui::Margin::symmetric(moduwu_design::SPACE_SM as i8, 6))
                .show(ui, |ui| TopBar::show_portrait(ui, app, frame));
        });
}

fn show_landscape(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
    let canvas = ui.available_rect_before_wrap();
    let context = ui.ctx().clone();
    let top = Rect::from_min_size(
        canvas.min,
        egui::vec2(canvas.width(), moduwu_design::TOOLBAR_HEIGHT + 12.0),
    );
    show_top_controls(&context, top, app, frame);
    let rail_width = Sidebar::ANDROID_LANDSCAPE_TOOL_RAIL_WIDTH;
    let rail = Rect::from_min_max(
        egui::pos2(canvas.right() - rail_width, top.bottom()),
        canvas.max,
    );
    let tools_width = if app.develop_ui.sidebar_open {
        ScreenLayout::Horizontal
            .sidebar_default_size(canvas.size())
            .min(canvas.width() * 0.48)
    } else {
        0.0
    };
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
    if app.develop_ui.sidebar_open {
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
                if Sidebar::shows_mask_strip(app) {
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
    }
    let exposed = Rect::from_min_max(egui::pos2(canvas.left(), top.bottom()), tools.left_bottom());
    app.show_preview(ui, frame, Some(exposed));
}
