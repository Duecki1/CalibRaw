use crate::pipeline::{PointColor, PointColorRange, PointColors, MAX_POINT_COLORS};
use crate::ui::components::adjustment_slider::{AdjustmentSlider, SliderGradient};
use crate::ui::components::color_picker::sidebar_color_picker;
use crate::ui::components::feathered_range::{self, FeatheredRange, RangeHandle};
use eframe::egui::{self, Color32, Mesh, Rect, Sense, Shape, Stroke, StrokeKind, Ui};
use egui_phosphor::regular;
use moduwu_design::NumberField;

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
    moduwu_design::toolbar_row(ui, |ui| {
        ui.add_enabled_ui(colors.len() < MAX_POINT_COLORS, |ui| {
            if moduwu_design::icon_toggle_button(
                ui,
                regular::EYEDROPPER,
                state.picker_active,
                moduwu_design::toolbar_icon_size(),
                "Sample a color from the image",
            )
            .clicked()
            {
                state.picker_active = !state.picker_active;
            }
        });
        ui.weak(format!("{} / {} colors", colors.len(), MAX_POINT_COLORS));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            delete = moduwu_design::icon_button_enabled(
                ui,
                !colors.is_empty(),
                regular::TRASH,
                moduwu_design::toolbar_icon_size(),
                "Delete selected color",
            )
            .clicked();
            reset = moduwu_design::icon_button_enabled(
                ui,
                !colors.is_empty(),
                regular::ARROW_COUNTER_CLOCKWISE,
                moduwu_design::toolbar_icon_size(),
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
            let visual = moduwu_design::interaction_visuals(ui, &response, selected);
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
                    if moduwu_design::menu_item(ui, true, "Reset color").clicked() {
                        state.selected = index;
                        reset = true;
                        ui.close();
                    }
                    if moduwu_design::destructive_menu_item(ui, "Delete color").clicked() {
                        state.selected = index;
                        delete = true;
                        ui.close();
                    }
                    if moduwu_design::destructive_menu_item(ui, "Delete all colors").clicked() {
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
    ui.add_space(moduwu_design::SPACE_XS);
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
        AdjustmentSlider::new("Hue Shift", &mut point.hue_shift, -100.0..=100.0)
            .decimals(0)
            .step(1.0)
            .hover_text("Shift the selected colors around the hue wheel.")
            .accent(accent)
            .gradient(SliderGradient::HueDegrees { start: hue - 180.0, end: hue + 180.0 })
            .show(ui);
        AdjustmentSlider::new("Saturation Shift", &mut point.saturation_shift, -100.0..=100.0)
            .decimals(0)
            .step(1.0)
            .hover_text("Increase or reduce the intensity of the selected colors.")
            .accent(accent)
            .gradient(SliderGradient::Saturation(accent))
            .show(ui);
        AdjustmentSlider::new("Luminance Shift", &mut point.luminance_shift, -100.0..=100.0)
            .decimals(0)
            .step(1.0)
            .hover_text("Brighten or darken the selected colors.")
            .accent(accent)
            .gradient(SliderGradient::Luminance(accent))
            .show(ui);
        adjusted_color_readout(ui, point);
        AdjustmentSlider::new("Range", &mut point.range, 0.0..=100.0)
            .decimals(0)
            .step(1.0)
            .hover_text("Widen or narrow all three selection ranges together.")
            .reset_to(50.0)
            .show(ui);
        let mut feather = point_color_feather(point);
        if AdjustmentSlider::new("Feather", &mut feather, 0.0..=100.0)
            .decimals(0)
            .step(1.0)
            .hover_text("Control how far the selection softly extends beyond the full-strength range. Increasing Feather only adds a wider soft falloff; it never shrinks the fully selected core.")
            .reset_to(50.0)
            .show(ui) {
            set_point_color_feather(point, feather);
        }
        egui::CollapsingHeader::new("Refine range").show(ui, |ui| {
            ui.weak("Circles move each soft edge of the selection; diamonds set where it reaches full strength.");
            let sample = point.sample_hsl;
            range_editor(ui, "Hue range", &mut point.hue_range, sample, 0);
            range_editor(ui, "Saturation range", &mut point.saturation_range, sample, 1);
            range_editor(ui, "Luminance range", &mut point.luminance_range, sample, 2);
        });
        if moduwu_design::toggle_button(ui, "Visualize range", state.visualize_range)
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
        (point.sample_hsl[1] * (1.0 + saturation / 100.0).max(0.0)).clamp(0.0, 1.0),
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
    moduwu_design::property_row(ui, "Adjusted color", |ui| {
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

fn gradient(
    painter: &egui::Painter,
    rect: Rect,
    columns: usize,
    rows: usize,
    color: impl Fn(f32, f32) -> Color32,
) {
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
    painter.add(Shape::mesh(mesh));
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

/// `value` limited to `low..=high` without panicking on an inverted or
/// non-finite bound, which an unsanitized saved range could produce.
fn limit_to(value: f32, low: f32, high: f32) -> f32 {
    value.max(low).min(high)
}

/// A Point Color channel range on the shared feathered-range track. The
/// track spans `-limit..=limit` around the sampled value. Like the depth
/// range, each soft edge is shown by its centre (circle) and the point where
/// it reaches full strength (diamond); moving a diamond widens or narrows
/// that edge symmetrically about its centre.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RangeTrack {
    range: PointColorRange,
    limit: f32,
    default: PointColorRange,
}

impl RangeTrack {
    fn to_track(self, value: f32) -> f32 {
        (value / self.limit + 1.0) * 0.5
    }

    fn start_center(self) -> f32 {
        (self.range.min + self.range.inner_min) * 0.5
    }

    fn end_center(self) -> f32 {
        (self.range.inner_max + self.range.max) * 0.5
    }
}

impl FeatheredRange for RangeTrack {
    fn handle_value(&self, handle: RangeHandle) -> f32 {
        self.to_track(match handle {
            RangeHandle::Start => self.start_center(),
            RangeHandle::End => self.end_center(),
            RangeHandle::StartFeather => self.range.inner_min,
            RangeHandle::EndFeather => self.range.inner_max,
        })
    }

    /// An edge centred on the end of the domain has no room to soften.
    fn handle_active(&self, handle: RangeHandle) -> bool {
        match handle {
            RangeHandle::Start | RangeHandle::End => true,
            RangeHandle::StartFeather => self.start_center() > -self.limit,
            RangeHandle::EndFeather => self.end_center() < self.limit,
        }
    }

    /// Keeps `min <= inner_min <= inner_max <= max` within the domain. An edge
    /// moved toward the domain end or the core narrows instead of crossing it.
    fn drag(&mut self, start: &Self, handle: RangeHandle, delta: f32) {
        let limit = start.limit;
        let shift = delta * 2.0 * limit;
        let r = start.range;
        let mut next = r;
        match handle {
            RangeHandle::Start => {
                let center = limit_to(start.start_center() + shift, -limit, r.inner_max);
                let half = ((r.inner_min - r.min) * 0.5)
                    .min(center + limit)
                    .min(r.inner_max - center)
                    .max(0.0);
                next.min = center - half;
                next.inner_min = center + half;
            }
            RangeHandle::End => {
                let center = limit_to(start.end_center() + shift, r.inner_min, limit);
                let half = ((r.max - r.inner_max) * 0.5)
                    .min(limit - center)
                    .min(center - r.inner_min)
                    .max(0.0);
                next.inner_max = center - half;
                next.max = center + half;
            }
            RangeHandle::StartFeather => {
                let center = start.start_center();
                let full = limit_to(
                    r.inner_min + shift,
                    center,
                    r.inner_max.min(2.0 * center + limit),
                );
                next.inner_min = full;
                next.min = 2.0 * center - full;
            }
            RangeHandle::EndFeather => {
                let center = start.end_center();
                let full = limit_to(
                    r.inner_max + shift,
                    r.inner_min.max(2.0 * center - limit),
                    center,
                );
                next.inner_max = full;
                next.max = 2.0 * center - full;
            }
        }
        self.range = next;
    }

    fn weight(&self, t: f32) -> f32 {
        self.range.weight((t * 2.0 - 1.0) * self.limit)
    }

    fn reset(&mut self) {
        self.range = self.default;
    }
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
        let defaults = PointColor::default();
        let default = [
            defaults.hue_range,
            defaults.saturation_range,
            defaults.luminance_range,
        ][axis];
        let mut track = RangeTrack {
            range: *range,
            limit,
            default,
        };
        let value_text = format!(
            "fade in {:.1}, full from {:.1}, full to {:.1}, fade out {:.1}",
            range.min * 100.0,
            range.inner_min * 100.0,
            range.inner_max * 100.0,
            range.max * 100.0
        );
        let outline = ui.visuals().widgets.noninteractive.bg_stroke;
        feathered_range::feathered_range_track(
            ui,
            &mut track,
            label,
            value_text,
            "Drag the lower circles to move where the selection softens. Drag the upper diamonds to set where it reaches full strength. Double-click to reset.",
            move |painter, bar| {
                gradient(painter, bar, 80, 1, |x, _| {
                    let offset = (x * 2.0 - 1.0) * limit;
                    let mut hsl = sample;
                    hsl[axis] = if axis == 0 {
                        (sample[axis] + offset).rem_euclid(1.0)
                    } else {
                        (sample[axis] + offset).clamp(0.0, 1.0)
                    };
                    hsl_color(hsl)
                });
                painter.rect_stroke(bar, 2.0, outline, StrokeKind::Inside);
            },
        );
        *range = track.range;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for (index, name) in ["Fade in", "Full from", "Full to", "Fade out"]
                .into_iter()
                .enumerate()
            {
                let mut value =
                    [range.min, range.inner_min, range.inner_max, range.max][index] * 100.0;
                let response = ui
                    .add(
                        NumberField::new(&mut value, -limit * 100.0..=limit * 100.0)
                            .speed(0.5)
                            .decimals(1),
                    )
                    .on_hover_text(format!("{name}: offset from sampled color"));
                if response.changed() {
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

    fn track(range: PointColorRange, limit: f32) -> RangeTrack {
        RangeTrack {
            range,
            limit,
            default: range,
        }
    }

    fn assert_ordered_in_domain(range: PointColorRange, limit: f32, context: &str) {
        assert!(-limit - 1e-6 <= range.min, "{context}: {range:?}");
        assert!(range.min <= range.inner_min + 1e-6, "{context}: {range:?}");
        assert!(
            range.inner_min <= range.inner_max + 1e-6,
            "{context}: {range:?}"
        );
        assert!(range.inner_max <= range.max + 1e-6, "{context}: {range:?}");
        assert!(range.max <= limit + 1e-6, "{context}: {range:?}");
    }

    #[test]
    fn track_handles_sit_at_edge_centres_and_full_strength_points() {
        let t = track(PointColorRange::new(-0.4, -0.2, 0.1, 0.3), 0.5);
        let at = |value: f32| (value / 0.5 + 1.0) * 0.5;
        assert!((t.handle_value(RangeHandle::Start) - at(-0.3)).abs() < 1e-6);
        assert!((t.handle_value(RangeHandle::End) - at(0.2)).abs() < 1e-6);
        assert!((t.handle_value(RangeHandle::StartFeather) - at(-0.2)).abs() < 1e-6);
        assert!((t.handle_value(RangeHandle::EndFeather) - at(0.1)).abs() < 1e-6);
        for x in 0..=20 {
            let offset = x as f32 / 20.0 - 0.5;
            assert_eq!(t.weight(at(offset)), t.range.weight(offset));
        }
    }

    #[test]
    fn dragging_keeps_the_range_ordered_and_in_its_domain() {
        for limit in [0.5, 1.0] {
            for start in [
                PointColorRange::new(-0.4, -0.2, 0.2, 0.4),
                PointColorRange::new(-0.2, 0.1, 0.1, 0.3),
                PointColorRange::new(-limit, -limit, limit, limit),
                PointColorRange::new(0.0, 0.0, 0.0, 0.0),
            ] {
                let start = track(start, limit);
                for handle in RangeHandle::ALL {
                    for delta in [-2.0, -0.3, -0.05, 0.0, 0.05, 0.3, 2.0] {
                        let mut moved = start;
                        moved.drag(&start, handle, delta);
                        assert_ordered_in_domain(
                            moved.range,
                            limit,
                            &format!("{handle:?} {delta} from {:?}", start.range),
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn edge_drags_keep_their_width_and_diamonds_change_only_their_edge() {
        let start = track(PointColorRange::new(-0.4, -0.2, 0.2, 0.4), 1.0);
        let mut moved = start;
        moved.drag(&start, RangeHandle::Start, -0.05);
        assert!((moved.range.min - -0.5).abs() < 1e-6);
        assert!((moved.range.inner_min - -0.3).abs() < 1e-6);
        assert_eq!(
            (moved.range.inner_max, moved.range.max),
            (start.range.inner_max, start.range.max)
        );

        let mut moved = start;
        moved.drag(&start, RangeHandle::EndFeather, -0.025);
        assert!((moved.range.inner_max - 0.15).abs() < 1e-6);
        assert!((moved.range.max - 0.45).abs() < 1e-6);
        assert_eq!(
            (moved.range.min, moved.range.inner_min),
            (start.range.min, start.range.inner_min)
        );
    }

    #[test]
    fn double_click_reset_restores_the_channel_default() {
        let mut t = RangeTrack {
            range: PointColorRange::new(-0.1, 0.0, 0.0, 0.1),
            limit: 1.0,
            default: PointColor::default().saturation_range,
        };
        t.reset();
        assert_eq!(t.range, PointColor::default().saturation_range);
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
