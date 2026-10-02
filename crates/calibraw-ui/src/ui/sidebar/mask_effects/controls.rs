use super::{effect_slider, egui, Ui};
use crate::pipeline::effect_params::FloatParamSpec;

/// Keep the detail sliders full-width even in narrow portrait cards.
pub(super) fn effect_details(ui: &mut Ui, label: &str, body: impl FnOnce(&mut Ui) -> bool) -> bool {
    ui.add_space(crate::ui::theme::SPACE_XS);
    egui::CollapsingHeader::new(label)
        .default_open(false)
        .show_unindented(ui, body)
        .body_returned
        .unwrap_or(false)
}

pub(super) fn effect_position(
    ui: &mut Ui,
    label: &str,
    position: &mut [f32; 2],
    specs: [FloatParamSpec; 2],
) -> bool {
    ui.push_id(label, |ui| {
        ui.label(label);
        let (_, mut changed) = position_pad(ui, label, position, specs);
        changed |= effect_details(ui, "Precise position", |ui| {
            ui.small("Position in percent: left/top is 0, right/bottom is 100.");
            effect_slider(ui, &mut position[0], specs[0])
                | effect_slider(ui, &mut position[1], specs[1])
        });
        changed
    })
    .inner
}

/// The pad represents the effect's coordinate frame (0–100%). Precise controls retain the
/// full parameter ranges, including off-image origins, without clamping them
/// just because the pad is drawn.
pub(super) fn position_pad(
    ui: &mut Ui,
    label: &str,
    position: &mut [f32; 2],
    specs: [FloatParamSpec; 2],
) -> (egui::Response, bool) {
    let width = ui.available_width().max(1.0);
    let height = (width * 0.45).clamp(80.0, 116.0);
    let (rect, mut response) =
        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, ui.is_enabled(), label));
    let canvas = rect.shrink(8.0_f32.min(width * 0.1));
    let before = *position;
    if response.double_clicked() {
        *position = [specs[0].default, specs[1].default];
    } else if response.clicked() || response.dragged() {
        response.request_focus();
        if let Some(pointer) = response.interact_pointer_pos() {
            position[0] = ((pointer.x - canvas.left()) / canvas.width() * 100.0)
                .clamp(specs[0].min.max(0.0), specs[0].max.min(100.0));
            position[1] = ((pointer.y - canvas.top()) / canvas.height() * 100.0)
                .clamp(specs[1].min.max(0.0), specs[1].max.min(100.0));
        }
    }
    if response.has_focus() && ui.is_enabled() {
        ui.input(|input| {
            let step = if input.modifiers.shift { 10.0 } else { 1.0 };
            for (axis, negative, positive) in [
                (0, egui::Key::ArrowLeft, egui::Key::ArrowRight),
                (1, egui::Key::ArrowUp, egui::Key::ArrowDown),
            ] {
                let delta =
                    i32::from(input.key_pressed(positive)) - i32::from(input.key_pressed(negative));
                if delta != 0 {
                    position[axis] = (position[axis] + delta as f32 * step)
                        .clamp(specs[axis].min, specs[axis].max);
                }
            }
        });
    }
    let changed = *position != before;
    if changed {
        response.mark_changed();
    }
    let visuals = ui.style().interact(&response);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);
    painter.rect_stroke(rect, 6.0, visuals.bg_stroke, egui::StrokeKind::Inside);
    let grid_stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    painter.rect_stroke(canvas, 2.0, grid_stroke, egui::StrokeKind::Inside);
    for fraction in [0.25, 0.5, 0.75] {
        let x = egui::lerp(canvas.x_range(), fraction);
        let y = egui::lerp(canvas.y_range(), fraction);
        painter.line_segment(
            [egui::pos2(x, canvas.top()), egui::pos2(x, canvas.bottom())],
            grid_stroke,
        );
        painter.line_segment(
            [egui::pos2(canvas.left(), y), egui::pos2(canvas.right(), y)],
            grid_stroke,
        );
    }
    let marker = egui::pos2(
        egui::lerp(canvas.x_range(), (position[0] / 100.0).clamp(0.0, 1.0)),
        egui::lerp(canvas.y_range(), (position[1] / 100.0).clamp(0.0, 1.0)),
    );
    let marker_color = if ui.is_enabled() {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().weak_text_color()
    };
    painter.circle_filled(marker, 5.0, marker_color);
    painter.circle_stroke(marker, 7.0, egui::Stroke::new(1.0, marker_color));
    if position.iter().any(|value| !(0.0..=100.0).contains(value)) {
        painter.text(
            canvas.center(),
            egui::Align2::CENTER_CENTER,
            "Position outside frame",
            egui::TextStyle::Small.resolve(ui.style()),
            ui.visuals().text_color(),
        );
    }
    let response = response.on_hover_text(format!(
        "{label}: {:.1}%, {:.1}%\nDrag to position. Arrow keys move 1%; Shift moves 10%.\nDouble-click to reset. Use Precise position to enter coordinates.",
        position[0], position[1],
    ));
    (response, changed)
}
