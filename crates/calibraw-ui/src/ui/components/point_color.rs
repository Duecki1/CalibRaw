use crate::pipeline::{PointColor, PointColorRange, PointColors, MAX_POINT_COLORS};
use crate::ui::components::adjustment_slider::{
    accented_gradient_adjustment_slider, adjustment_slider_with_reset, SliderGradient,
};
use crate::ui::{icons, theme};
use eframe::egui::{self, Color32, Mesh, Rect, Sense, Shape, Stroke, StrokeKind, Ui};
use egui_phosphor::regular;

#[derive(Clone, Debug, Default)]
pub(crate) struct PointColorUiState {
    pub(crate) selected: usize,
    pub(crate) picker_active: bool,
    pub(crate) visualize_range: bool,
}

pub(crate) fn point_color(
    ui: &mut Ui,
    colors: &mut PointColors,
    state: &mut PointColorUiState,
) -> bool {
    let before = *colors;
    state.selected = state.selected.min(colors.len().saturating_sub(1));
    if colors.len() == MAX_POINT_COLORS {
        state.picker_active = false;
    }
    if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        state.picker_active = false;
    }
    let mut reset = false;
    let mut delete = false;
    theme::toolbar_row(ui, |ui| {
        ui.add_enabled_ui(colors.len() < MAX_POINT_COLORS, |ui| {
            if icons::phosphor_icon_toggle_button(
                ui,
                regular::EYEDROPPER,
                state.picker_active,
                theme::toolbar_icon_size(),
                "Sample a color from the image",
            )
            .clicked()
            {
                state.picker_active = !state.picker_active;
            }
        });
        ui.weak(format!("{} / {} colors", colors.len(), MAX_POINT_COLORS));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            delete = icons::phosphor_icon_button_enabled(
                ui,
                !colors.is_empty(),
                regular::TRASH,
                theme::toolbar_icon_size(),
                "Delete selected color",
            )
            .clicked();
            reset = icons::phosphor_icon_button_enabled(
                ui,
                !colors.is_empty(),
                regular::ARROW_COUNTER_CLOCKWISE,
                theme::toolbar_icon_size(),
                "Reset this color's adjustments and ranges",
            )
            .clicked();
        });
    });
    let width = ui.available_width().max(0.0);
    let gap = 4.0_f32.min(width / 16.0);
    let cell_width = ((width - gap * 7.0) / 8.0).max(0.0);
    let mut delete_all = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for index in 0..MAX_POINT_COLORS {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(cell_width, 30.0), Sense::click());
            let selected = index == state.selected && index < colors.len();
            let visual = theme::interaction_visuals(ui, &response, selected);
            ui.painter().rect_filled(rect, 4.0, visual.weak_fill);
            ui.painter()
                .rect_stroke(rect, 4.0, visual.stroke, StrokeKind::Inside);
            if let Some(color) = colors.get(index) {
                let radius = (cell_width * 0.27).min(9.0);
                ui.painter()
                    .circle_filled(rect.center(), radius, rgb_color(color.sample_rgb()));
                ui.painter().circle_stroke(
                    rect.center(),
                    radius,
                    Stroke::new(1.0, Color32::from_white_alpha(100)),
                );
                if response.clicked() {
                    state.selected = index;
                }
                response.context_menu(|ui| {
                    if ui.button("Reset color").clicked() {
                        state.selected = index;
                        reset = true;
                        ui.close();
                    }
                    if ui.button("Delete color").clicked() {
                        state.selected = index;
                        delete = true;
                        ui.close();
                    }
                    if ui.button("Delete all colors").clicked() {
                        delete_all = true;
                        ui.close();
                    }
                });
            } else if index == colors.len() {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    regular::PLUS,
                    egui::FontId::proportional(14.0),
                    ui.visuals().weak_text_color(),
                );
                if response.on_hover_text("Sample another color").clicked() {
                    state.picker_active = true;
                }
            }
        }
    });
    if delete_all {
        colors.clear();
    } else if delete {
        colors.remove(state.selected);
    }
    state.selected = state.selected.min(colors.len().saturating_sub(1));
    if reset {
        if let Some(point) = colors.get_mut(state.selected) {
            let sample = point.sample_hsl;
            *point = PointColor::default();
            point.sample_hsl = sample;
        }
    }
    ui.add_space(theme::SPACE_XS);
    if state.picker_active {
        ui.weak("Click a color in the image. Esc to cancel.");
    }
    let Some(point) = colors.get_mut(state.selected) else {
        state.visualize_range = false;
        if !state.picker_active {
            ui.weak("Use the eyedropper to select a color in your photo.");
        }
        return before != *colors;
    };
    ui.push_id(("point-color-controls", state.selected), |ui| {
        point_picker(ui, point);
        let accent = rgb_color(point.sample_rgb());
        let hue = point.sample_hsl[0] * 360.0;
        accented_gradient_adjustment_slider(ui, "Hue Shift", &mut point.hue_shift, -100.0..=100.0,
            0, 1.0, Some("Shift the selected colors around the hue wheel."), accent,
            SliderGradient::HueDegrees { start: hue - 180.0, end: hue + 180.0 });
        accented_gradient_adjustment_slider(ui, "Saturation Shift", &mut point.saturation_shift, -100.0..=100.0,
            0, 1.0, Some("Increase or reduce the intensity of the selected colors."), accent,
            SliderGradient::Saturation(accent));
        accented_gradient_adjustment_slider(ui, "Luminance Shift", &mut point.luminance_shift, -100.0..=100.0,
            0, 1.0, Some("Brighten or darken the selected colors."), accent,
            SliderGradient::Luminance(accent));
        adjustment_slider_with_reset(ui, "Range", &mut point.range, 0.0..=100.0, 0, 1.0,
            Some("Widen or narrow all three selection ranges together."), 50.0);
        let mut feather = point_color_feather(point);
        if adjustment_slider_with_reset(ui, "Feather", &mut feather, 0.0..=100.0, 0, 1.0,
            Some("Control how gradually the selection fades at its edges. This keeps the outer range fixed and moves the inner full-strength boundaries for hue, saturation, and luminance together."), 50.0) {
            set_point_color_feather(point, feather);
        }
        egui::CollapsingHeader::new("Refine range").show(ui, |ui| {
            ui.weak("Outer handles set the selection limits; inner handles set where the feather reaches full strength.");
            let sample = point.sample_hsl;
            range_editor(ui, "Hue range", &mut point.hue_range, sample, 0);
            range_editor(ui, "Saturation range", &mut point.saturation_range, sample, 1);
            range_editor(ui, "Luminance range", &mut point.luminance_range, sample, 2);
        });
        if theme::toggle_button(ui, "Visualize range", state.visualize_range)
            .on_hover_text("Show the selected range with the same blue translucent overlay used for masks. Feathered pixels use a softer overlay. This preview is never exported.")
            .clicked()
        {
            state.visualize_range = !state.visualize_range;
        }
    });
    before != *colors
}

fn adjusted_hsl(point: &PointColor, hue: f32, saturation: f32, luminance: f32) -> [f32; 3] {
    [
        (point.sample_hsl[0] + hue / 200.0).rem_euclid(1.0),
        (point.sample_hsl[1] + saturation / 100.0).clamp(0.0, 1.0),
        (point.sample_hsl[2] + luminance / 100.0).clamp(0.0, 1.0),
    ]
}

fn hsl_color(hsl: [f32; 3]) -> Color32 {
    let point = PointColor {
        sample_hsl: hsl,
        ..PointColor::default()
    };
    rgb_color(point.sample_rgb())
}

fn gradient(ui: &Ui, rect: Rect, columns: usize, rows: usize, color: impl Fn(f32, f32) -> Color32) {
    let mut mesh = Mesh::default();
    for y in 0..=rows {
        for x in 0..=columns {
            let u = x as f32 / columns as f32;
            let v = y as f32 / rows as f32;
            mesh.colored_vertex(
                egui::pos2(egui::lerp(rect.x_range(), u), egui::lerp(rect.y_range(), v)),
                color(u, v),
            );
        }
    }
    for y in 0..rows {
        for x in 0..columns {
            let a = (y * (columns + 1) + x) as u32;
            let b = a + columns as u32 + 1;
            mesh.add_triangle(a, a + 1, b);
            mesh.add_triangle(a + 1, b + 1, b);
        }
    }
    ui.painter().add(Shape::mesh(mesh));
}

fn point_picker(ui: &mut Ui, point: &mut PointColor) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(1.0), 112.0),
        Sense::hover(),
    );
    let strip_width = (rect.width() * 0.09).clamp(1.0, 22.0);
    let gap = 8.0_f32.min(rect.width() * 0.05);
    let plane = Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right() - strip_width - gap, rect.bottom()),
    );
    let strip = Rect::from_min_max(egui::pos2(rect.right() - strip_width, rect.top()), rect.max);
    let plane_response = ui
        .interact(
            plane,
            ui.id().with("hue-saturation-plane"),
            Sense::click_and_drag(),
        )
        .on_hover_text("Drag to shift hue and saturation. Double-click to reset both.");
    let strip_response = ui
        .interact(
            strip,
            ui.id().with("luminance-strip"),
            Sense::click_and_drag(),
        )
        .on_hover_text("Drag to shift luminance. Double-click to reset.");
    if plane_response.double_clicked() {
        point.hue_shift = 0.0;
        point.saturation_shift = 0.0;
    } else if plane_response.clicked() || plane_response.dragged() {
        if let Some(pos) = plane_response.interact_pointer_pos() {
            point.hue_shift =
                (((pos.x - plane.left()) / plane.width()).clamp(0.0, 1.0) - 0.5) * 200.0;
            point.saturation_shift =
                (0.5 - ((pos.y - plane.top()) / plane.height()).clamp(0.0, 1.0)) * 200.0;
        }
    }
    if strip_response.double_clicked() {
        point.luminance_shift = 0.0;
    } else if strip_response.clicked() || strip_response.dragged() {
        if let Some(pos) = strip_response.interact_pointer_pos() {
            point.luminance_shift =
                (0.5 - ((pos.y - strip.top()) / strip.height()).clamp(0.0, 1.0)) * 200.0;
        }
    }
    gradient(ui, plane, 48, 12, |x, y| {
        hsl_color(adjusted_hsl(
            point,
            (x - 0.5) * 200.0,
            (0.5 - y) * 200.0,
            point.luminance_shift,
        ))
    });
    gradient(ui, strip, 1, 32, |_, y| {
        hsl_color(adjusted_hsl(
            point,
            point.hue_shift,
            point.saturation_shift,
            (0.5 - y) * 200.0,
        ))
    });
    for area in [plane, strip] {
        ui.painter().rect_stroke(
            area,
            2.0,
            ui.visuals().widgets.noninteractive.bg_stroke,
            StrokeKind::Inside,
        );
    }
    ui.painter().circle_stroke(
        plane.center(),
        3.0,
        Stroke::new(1.0, Color32::from_white_alpha(130)),
    );
    let marker = egui::pos2(
        egui::lerp(plane.x_range(), 0.5 + point.hue_shift / 200.0),
        egui::lerp(plane.y_range(), 0.5 - point.saturation_shift / 200.0),
    );
    ui.painter()
        .circle_stroke(marker, 6.0, Stroke::new(3.0, Color32::BLACK));
    ui.painter()
        .circle_stroke(marker, 6.0, Stroke::new(1.5, Color32::WHITE));
    let y = egui::lerp(strip.y_range(), 0.5 - point.luminance_shift / 200.0);
    let line = [egui::pos2(strip.left(), y), egui::pos2(strip.right(), y)];
    ui.painter()
        .line_segment(line, Stroke::new(4.0, Color32::BLACK));
    ui.painter()
        .line_segment(line, Stroke::new(2.0, Color32::WHITE));
}

fn range_feather(range: PointColorRange) -> f32 {
    let width = range.max - range.min;
    if !width.is_finite() || width <= f32::EPSILON {
        return 0.0;
    }
    (((range.inner_min - range.min) + (range.max - range.inner_max)) / width).clamp(0.0, 1.0)
}

fn point_color_feather(point: &PointColor) -> f32 {
    let feather = range_feather(point.hue_range)
        + range_feather(point.saturation_range)
        + range_feather(point.luminance_range);
    feather / 3.0 * 100.0
}

fn set_range_feather(range: &mut PointColorRange, feather: f32) {
    let amount = (feather / 100.0).clamp(0.0, 1.0);
    let center = if range.min <= 0.0 && range.max >= 0.0 {
        0.0
    } else {
        (range.min + range.max) * 0.5
    };
    range.inner_min = egui::lerp(range.min..=center, amount);
    range.inner_max = egui::lerp(range.max..=center, amount);
}

fn set_point_color_feather(point: &mut PointColor, feather: f32) {
    set_range_feather(&mut point.hue_range, feather);
    set_range_feather(&mut point.saturation_range, feather);
    set_range_feather(&mut point.luminance_range, feather);
}

fn set_range_handle(range: &mut PointColorRange, index: usize, value: f32, limit: f32) {
    let mut values = [range.min, range.inner_min, range.inner_max, range.max];
    let low = if index == 0 {
        -limit
    } else {
        values[index - 1]
    };
    let high = if index == 3 { limit } else { values[index + 1] };
    values[index] = value.clamp(low, high);
    *range = PointColorRange::new(values[0], values[1], values[2], values[3]);
}

fn range_editor(
    ui: &mut Ui,
    label: &str,
    range: &mut PointColorRange,
    sample: [f32; 3],
    axis: usize,
) {
    ui.push_id(label, |ui| {
        ui.label(label);
        let limit = if axis == 0 { 0.5 } else { 1.0 };
        let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 42.0), Sense::click_and_drag());
        let bar = rect.shrink2(egui::vec2(7.0, 10.0));
        let values = [range.min, range.inner_min, range.inner_max, range.max];
        let handle_pos = |value: f32| egui::lerp(bar.x_range(), (value / limit + 1.0) * 0.5);
        let drag_id = ui.id().with("active-range-handle");
        if response.drag_started() || response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let index = values.iter().enumerate().min_by(|(a, x), (b, y)| {
                    let distance = |index: usize, value: f32| {
                        let dy = if index == 0 || index == 3 { rect.bottom() } else { rect.top() };
                        egui::pos2(handle_pos(value), dy).distance_sq(pos)
                    };
                    distance(*a, **x).total_cmp(&distance(*b, **y))
                }).map(|(index, _)| index).unwrap_or(0);
                ui.ctx().data_mut(|data| data.insert_temp(drag_id, index));
            }
        }
        if response.dragged() || response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let index = ui.ctx().data(|data| data.get_temp::<usize>(drag_id)).unwrap_or(0);
                set_range_handle(range, index, ((pos.x - bar.left()) / bar.width() * 2.0 - 1.0) * limit, limit);
            }
        }
        gradient(ui, bar, 80, 1, |x, _| {
            let offset = (x * 2.0 - 1.0) * limit;
            let mut hsl = sample;
            hsl[axis] = if axis == 0 { (sample[axis] + offset).rem_euclid(1.0) } else { (sample[axis] + offset).clamp(0.0, 1.0) };
            let color = hsl_color(hsl);
            color.gamma_multiply(0.25 + 0.75 * range.weight(offset))
        });
        ui.painter().rect_stroke(bar, 2.0, ui.visuals().widgets.noninteractive.bg_stroke, StrokeKind::Inside);
        for (index, value) in [range.min, range.inner_min, range.inner_max, range.max].into_iter().enumerate() {
            let x = handle_pos(value);
            let y = if index == 0 || index == 3 { bar.bottom() + 4.0 } else { bar.top() - 4.0 };
            ui.painter().line_segment([egui::pos2(x, bar.top()), egui::pos2(x, bar.bottom())], Stroke::new(1.0, Color32::WHITE));
            ui.painter().circle_filled(egui::pos2(x, y), 4.0, ui.visuals().widgets.inactive.bg_fill);
            ui.painter().circle_stroke(egui::pos2(x, y), 4.0, ui.visuals().selection.stroke);
        }
        response.on_hover_text("Drag the lower outer handles for feathering and the upper inner handles for full strength.");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for (index, name) in ["Fade in", "Full from", "Full to", "Fade out"].into_iter().enumerate() {
                let mut value = [range.min, range.inner_min, range.inner_max, range.max][index] * 100.0;
                if ui.add(egui::DragValue::new(&mut value).range(-limit * 100.0..=limit * 100.0)
                    .speed(0.5).max_decimals(1)).on_hover_text(format!("{name}: offset from sampled color")).changed() {
                    set_range_handle(range, index, value / 100.0, limit);
                }
            }
        });
    });
}

fn rgb_color(rgb: [f32; 3]) -> Color32 {
    Color32::from_rgb(
        (rgb[0].clamp(0.0, 1.0) * 255.0) as u8,
        (rgb[1].clamp(0.0, 1.0) * 255.0) as u8,
        (rgb[2].clamp(0.0, 1.0) * 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_handles_cannot_cross_or_escape_the_domain() {
        for index in 0..4 {
            for value in [-5.0, -0.25, 0.0, 0.25, 5.0] {
                let mut range = PointColorRange::new(-0.4, -0.2, 0.2, 0.4);
                set_range_handle(&mut range, index, value, 0.5);
                assert!(-0.5 <= range.min && range.min <= range.inner_min);
                assert!(range.inner_min <= range.inner_max && range.inner_max <= range.max);
                assert!(range.max <= 0.5);
            }
        }
    }

    #[test]
    fn simple_feather_defaults_to_fifty_percent() {
        let point = PointColor::from_srgb([0.8, 0.3, 0.2]);
        assert!((point_color_feather(&point) - 50.0).abs() < 1e-5);
    }

    #[test]
    fn simple_feather_preserves_outer_bounds_and_moves_all_inner_handles() {
        let mut point = PointColor::from_srgb([0.8, 0.3, 0.2]);
        let hue_outer = (point.hue_range.min, point.hue_range.max);
        let saturation_outer = (point.saturation_range.min, point.saturation_range.max);
        let luminance_outer = (point.luminance_range.min, point.luminance_range.max);

        set_point_color_feather(&mut point, 0.0);
        assert_eq!((point.hue_range.min, point.hue_range.max), hue_outer);
        assert_eq!((point.saturation_range.min, point.saturation_range.max), saturation_outer);
        assert_eq!((point.luminance_range.min, point.luminance_range.max), luminance_outer);
        assert_eq!(point.hue_range.inner_min, point.hue_range.min);
        assert_eq!(point.hue_range.inner_max, point.hue_range.max);
        assert_eq!(point.saturation_range.inner_min, point.saturation_range.min);
        assert_eq!(point.saturation_range.inner_max, point.saturation_range.max);
        assert_eq!(point.luminance_range.inner_min, point.luminance_range.min);
        assert_eq!(point.luminance_range.inner_max, point.luminance_range.max);
        assert!((point_color_feather(&point) - 0.0).abs() < 1e-5);

        set_point_color_feather(&mut point, 100.0);
        assert_eq!((point.hue_range.min, point.hue_range.max), hue_outer);
        assert_eq!((point.saturation_range.min, point.saturation_range.max), saturation_outer);
        assert_eq!((point.luminance_range.min, point.luminance_range.max), luminance_outer);
        assert_eq!(point.hue_range.inner_min, 0.0);
        assert_eq!(point.hue_range.inner_max, 0.0);
        assert_eq!(point.saturation_range.inner_min, 0.0);
        assert_eq!(point.saturation_range.inner_max, 0.0);
        assert_eq!(point.luminance_range.inner_min, 0.0);
        assert_eq!(point.luminance_range.inner_max, 0.0);
        assert!((point_color_feather(&point) - 100.0).abs() < 1e-5);
    }

    #[test]
    fn component_preserves_edits_and_fits_a_narrow_panel_when_idle() {
        let ctx = egui::Context::default();
        let mut colors = PointColors::default();
        colors.push(PointColor::from_srgb([0.8, 0.3, 0.2]));
        let before = colors;
        let mut state = PointColorUiState::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(220.0);
            let left = ui.cursor().left();
            assert!(!point_color(ui, &mut colors, &mut state));
            assert!(ui.min_rect().right() <= left + 220.1);
        });
        assert_eq!(before, colors);
    }
}
