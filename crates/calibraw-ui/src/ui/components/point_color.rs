use crate::pipeline::{PointColor, PointColorRange, PointColors, MAX_POINT_COLORS};
use crate::ui::components::adjustment_slider::{
    accented_gradient_adjustment_slider, adjustment_slider_with_reset, SliderGradient,
};
use crate::ui::components::color_picker::sidebar_color_picker;
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
        let mut target_rgb = point.sample_rgb();
        if sidebar_color_picker(
            ui,
            "target-color",
            &mut target_rgb,
            "Target color",
            "Choose which color in the photo is affected",
        ) {
            set_target_color(point, target_rgb);
        }
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
        adjusted_color_readout(ui, point);
        adjustment_slider_with_reset(ui, "Range", &mut point.range, 0.0..=100.0, 0, 1.0,
            Some("Widen or narrow all three selection ranges together."), 50.0);
        let mut feather = point_color_feather(point);
        if adjustment_slider_with_reset(ui, "Feather", &mut feather, 0.0..=100.0, 0, 1.0,
            Some("Control how far the selection softly extends beyond the full-strength range. Increasing Feather only adds a wider soft falloff; it never shrinks the fully selected core."), 50.0) {
            set_point_color_feather(point, feather);
        }
        egui::CollapsingHeader::new("Refine range").show(ui, |ui| {
            ui.weak("Inner handles set the full-strength core; outer handles set how far the feathered selection extends.");
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

fn set_target_color(point: &mut PointColor, rgb: [f32; 3]) {
    point.sample_hsl = PointColor::from_srgb(rgb).sample_hsl;
}

fn adjusted_color_readout(ui: &mut Ui, point: &PointColor) {
    let color = hsl_color(adjusted_hsl(
        point,
        point.hue_shift,
        point.saturation_shift,
        point.luminance_shift,
    ));
    theme::property_row(ui, "Adjusted color", |ui| {
        ui.label(
            egui::RichText::new(format!(
                "#{:02X}{:02X}{:02X}",
                color.r(),
                color.g(),
                color.b()
            ))
            .monospace(),
        );
        let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::hover());
        ui.painter().rect_filled(rect, 4.0, color);
        ui.painter().rect_stroke(
            rect,
            4.0,
            ui.visuals().widgets.noninteractive.bg_stroke,
            StrokeKind::Inside,
        );
    });
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

fn range_feather(range: PointColorRange) -> f32 {
    let core_width = range.inner_max - range.inner_min;
    if !core_width.is_finite() || core_width <= f32::EPSILON {
        return 0.0;
    }
    let feather_width =
        (range.inner_min - range.min).max(0.0) + (range.max - range.inner_max).max(0.0);
    (feather_width / (2.0 * core_width)).clamp(0.0, 1.0)
}

fn point_color_feather(point: &PointColor) -> f32 {
    let feather = range_feather(point.hue_range)
        + range_feather(point.saturation_range)
        + range_feather(point.luminance_range);
    feather / 3.0 * 100.0
}

fn set_range_feather(range: &mut PointColorRange, feather: f32, limit: f32) {
    let amount = (feather / 100.0).clamp(0.0, 1.0);
    let core_width = (range.inner_max - range.inner_min).max(0.0);
    let feather_width = core_width * amount;
    range.min = (range.inner_min - feather_width).clamp(-limit, range.inner_min);
    range.max = (range.inner_max + feather_width).clamp(range.inner_max, limit);
}

fn set_point_color_feather(point: &mut PointColor, feather: f32) {
    set_range_feather(&mut point.hue_range, feather, 0.5);
    set_range_feather(&mut point.saturation_range, feather, 1.0);
    set_range_feather(&mut point.luminance_range, feather, 1.0);
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
        (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8,
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
    fn changing_target_color_keeps_adjustments_and_ranges() {
        let mut point = PointColor::from_srgb([0.8, 0.3, 0.2]);
        point.hue_shift = 15.0;
        point.saturation_shift = -20.0;
        point.luminance_shift = 8.0;
        point.range = 35.0;
        let before = point;

        set_target_color(&mut point, [0.2, 0.6, 0.9]);

        assert_eq!(
            point.sample_hsl,
            PointColor::from_srgb([0.2, 0.6, 0.9]).sample_hsl
        );
        assert_eq!(point.hue_shift, before.hue_shift);
        assert_eq!(point.saturation_shift, before.saturation_shift);
        assert_eq!(point.luminance_shift, before.luminance_shift);
        assert_eq!(point.range, before.range);
        assert_eq!(point.hue_range, before.hue_range);
        assert_eq!(point.saturation_range, before.saturation_range);
        assert_eq!(point.luminance_range, before.luminance_range);
    }

    #[test]
    fn simple_feather_preserves_core_and_expands_outer_bounds() {
        let mut point = PointColor::from_srgb([0.8, 0.3, 0.2]);
        let hue_core = (point.hue_range.inner_min, point.hue_range.inner_max);
        let saturation_core = (
            point.saturation_range.inner_min,
            point.saturation_range.inner_max,
        );
        let luminance_core = (
            point.luminance_range.inner_min,
            point.luminance_range.inner_max,
        );

        set_point_color_feather(&mut point, 0.0);
        assert_eq!(
            (point.hue_range.inner_min, point.hue_range.inner_max),
            hue_core
        );
        assert_eq!(
            (
                point.saturation_range.inner_min,
                point.saturation_range.inner_max
            ),
            saturation_core
        );
        assert_eq!(
            (
                point.luminance_range.inner_min,
                point.luminance_range.inner_max
            ),
            luminance_core
        );
        assert_eq!(point.hue_range.min, point.hue_range.inner_min);
        assert_eq!(point.hue_range.max, point.hue_range.inner_max);
        assert_eq!(point.saturation_range.min, point.saturation_range.inner_min);
        assert_eq!(point.saturation_range.max, point.saturation_range.inner_max);
        assert_eq!(point.luminance_range.min, point.luminance_range.inner_min);
        assert_eq!(point.luminance_range.max, point.luminance_range.inner_max);
        assert!((point_color_feather(&point) - 0.0).abs() < 1e-5);

        set_point_color_feather(&mut point, 100.0);
        assert_eq!(
            (point.hue_range.inner_min, point.hue_range.inner_max),
            hue_core
        );
        assert_eq!(
            (
                point.saturation_range.inner_min,
                point.saturation_range.inner_max
            ),
            saturation_core
        );
        assert_eq!(
            (
                point.luminance_range.inner_min,
                point.luminance_range.inner_max
            ),
            luminance_core
        );
        assert_eq!(point.hue_range.min, -0.1875);
        assert_eq!(point.hue_range.max, 0.1875);
        assert_eq!(point.saturation_range.min, -0.75);
        assert_eq!(point.saturation_range.max, 0.75);
        assert_eq!(point.luminance_range.min, -0.75);
        assert_eq!(point.luminance_range.max, 0.75);
        assert!((point_color_feather(&point) - 100.0).abs() < 1e-5);
    }

    #[test]
    fn increasing_feather_only_adds_selected_colors() {
        let mut point = PointColor::from_srgb([0.2, 0.5, 0.8]);
        set_point_color_feather(&mut point, 0.0);
        let hard = point.hue_range;
        let inside_core = hard.weight(0.04);
        let just_outside_core = hard.weight(0.09);

        set_point_color_feather(&mut point, 100.0);
        let soft = point.hue_range;
        assert_eq!(soft.inner_min, hard.inner_min);
        assert_eq!(soft.inner_max, hard.inner_max);
        assert_eq!(soft.weight(0.04), inside_core);
        assert!(soft.weight(0.09) > just_outside_core);
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
