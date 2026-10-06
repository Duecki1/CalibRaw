use crate::pipeline::{PointColor, PointColorRange, PointColors, MAX_POINT_COLORS};
use crate::ui::components::adjustment_slider::{AdjustmentSlider, SliderGradient};
use crate::ui::components::color_picker::sidebar_color_picker;
use crate::ui::components::feathered_range::{
    self, FeatheredRange, RangeEdit, RangeHandle, RangePoints,
};
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
            ui.weak("Solid handles move where the selection reaches full strength; hollow handles move where it starts to fade.");
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

/// A Point Color channel range on the shared feathered-range track. The
/// track spans `-limit..=limit` around the sampled value. Solid handles sit
/// where full strength starts (`inner_min`, `inner_max`) and move that edge;
/// hollow ones sit where the fade begins (`min`, `max`) and move only it.
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

    fn offset_at(self, t: f32) -> f32 {
        feathered_range::round_to((t * 2.0 - 1.0) * self.limit, 4)
    }
}

impl FeatheredRange for RangeTrack {
    fn points(&self) -> RangePoints {
        RangePoints {
            fade_in: self.to_track(self.range.min),
            full_from: self.to_track(self.range.inner_min),
            full_to: self.to_track(self.range.inner_max),
            fade_out: self.to_track(self.range.max),
        }
    }

    fn set_points(&mut self, edit: RangeEdit) {
        let mut range = self.range;
        let offset_at = |t| self.offset_at(t);
        if let Some(edit) = edit.start {
            let fade = range.inner_min - range.min;
            let (inner_min, min) = edit.resolve(range.inner_min, offset_at);
            range.inner_min = inner_min;
            range.min = min.unwrap_or(inner_min - fade);
        }
        if let Some(edit) = edit.end {
            let fade = range.max - range.inner_max;
            let (inner_max, max) = edit.resolve(range.inner_max, offset_at);
            range.inner_max = inner_max;
            range.max = max.unwrap_or(inner_max + fade);
        }
        self.range = valid_range(range, self.limit, edit.end.is_some());
    }

    fn handle_text(&self, handle: RangeHandle) -> String {
        let (name, value) = match handle {
            RangeHandle::StartFeather => ("Fade in", self.range.min),
            RangeHandle::Start => ("Full from", self.range.inner_min),
            RangeHandle::End => ("Full to", self.range.inner_max),
            RangeHandle::EndFeather => ("Fade out", self.range.max),
        };
        format!("{name} {:.1}", value * 100.0)
    }

    fn weight(&self, t: f32) -> f32 {
        self.range.weight((t * 2.0 - 1.0) * self.limit)
    }

    fn reset(&mut self) {
        self.range = self.default;
    }
}

/// `range` as sidecar validation requires it: every value within
/// `-limit..=limit` and `min <= inner_min <= inner_max <= max`, exactly.
/// `PointColorRange::new` checks neither, and rounding a moved edge to the
/// stored precision, or carrying a fade from an unrounded edge of older
/// settings, can leave a value a hair outside. The edge this step changed
/// yields (`end_changed`: the inner max), so an untouched edge stays exact.
fn valid_range(range: PointColorRange, limit: f32, end_changed: bool) -> PointColorRange {
    let [min, mut inner_min, mut inner_max, max] =
        [range.min, range.inner_min, range.inner_max, range.max].map(|v| v.clamp(-limit, limit));
    if inner_min > inner_max {
        if end_changed {
            inner_max = inner_min;
        } else {
            inner_min = inner_max;
        }
    }
    PointColorRange::new(min.min(inner_min), inner_min, inner_max, max.max(inner_max))
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
        let outline = ui.visuals().widgets.noninteractive.bg_stroke;
        feathered_range::feathered_range_track(
            ui,
            &mut track,
            label,
            "Drag a solid handle to move where the selection reaches full strength; drag a hollow handle to move where it starts to fade in or out.",
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

    fn track(range: PointColorRange, limit: f32) -> RangeTrack {
        RangeTrack {
            range,
            limit,
            default: range,
        }
    }

    /// Exactly the bounds `validate_point_color_range` checks before a
    /// sidecar is written, with no tolerance.
    fn assert_ordered_in_domain(range: PointColorRange, limit: f32, context: &str) {
        let values = [range.min, range.inner_min, range.inner_max, range.max];
        assert!(
            values.iter().all(|value| (-limit..=limit).contains(value)),
            "{context}: {range:?}"
        );
        assert!(
            values.windows(2).all(|pair| pair[0] <= pair[1]),
            "{context}: {range:?}"
        );
    }

    /// Ranges to drag from: rounded ones, ones at the domain edges, and
    /// unrounded ones as the simple Feather slider and older settings leave.
    fn drag_starts(limit: f32) -> Vec<PointColorRange> {
        let mut feathered = PointColorRange::new(0.0, -0.123_456_7, 0.234_567_8, 0.0);
        set_range_feather(&mut feathered, 37.3, limit);
        vec![
            PointColorRange::new(-0.4, -0.2, 0.2, 0.4),
            PointColorRange::new(-0.2, 0.1, 0.1, 0.3),
            PointColorRange::new(-limit, -limit, limit, limit),
            PointColorRange::new(0.0, 0.0, 0.0, 0.0),
            PointColorRange::new(-0.333_333_3, -0.111_111_1, 0.111_111_1, 0.333_333_3),
            PointColorRange::new(-limit, -0.000_049_9, 0.000_049_9, limit),
            feathered,
        ]
    }

    /// Every handle dragged by `delta` from each start, for one channel.
    fn dragged_ranges(limit: f32) -> Vec<(String, PointColorRange)> {
        let mut ranges = Vec::new();
        for start in drag_starts(limit) {
            let start = track(start, limit);
            for handle in RangeHandle::ALL {
                for delta in [-2.0, -0.3, -0.05, -0.000_03, 0.0, 0.000_03, 0.05, 0.3, 2.0] {
                    let mut moved = start;
                    feathered_range::drag_range(&mut moved, &start, handle, delta);
                    let context = format!("{handle:?} {delta} from {:?}", start.range);
                    ranges.push((context, moved.range));
                }
                // Onto the other edge's exact place, where only one side is
                // rounded.
                let other = match handle {
                    RangeHandle::Start => RangeHandle::End,
                    RangeHandle::End => RangeHandle::Start,
                    _ => continue,
                };
                let mut moved = start;
                let mut drag = feathered_range::HandleDrag::new(&start, handle);
                drag.move_to(&mut moved, start.points().get(other));
                ranges.push((format!("{handle:?} onto {other:?}"), moved.range));
            }
        }
        ranges
    }

    #[test]
    fn dragging_keeps_the_range_ordered_and_in_its_domain() {
        for limit in [0.5, 1.0] {
            for (context, range) in dragged_ranges(limit) {
                assert_ordered_in_domain(range, limit, &context);
            }
        }
    }

    #[test]
    fn dragged_ranges_pass_sidecar_validation() {
        let hue = dragged_ranges(0.5);
        let channel = dragged_ranges(1.0);
        for (index, ((context, hue), (_, channel))) in hue.iter().zip(&channel).enumerate() {
            let mut point = PointColor::from_srgb([0.8, 0.3, 0.2]);
            point.hue_range = *hue;
            point.saturation_range = *channel;
            point.luminance_range = *channel;
            let mut edits = crate::sidecar::default_edit_state();
            assert!(edits.exposure.point_colors.push(point));
            if let Err(error) = crate::sidecar::encode(edits) {
                panic!("{index} {context}: {error:?}");
            }
        }
    }

    #[test]
    fn solid_handles_move_their_edge_and_fade_handles_only_the_fade() {
        let start = track(PointColorRange::new(-0.4, -0.2, 0.2, 0.4), 1.0);
        let mut moved = start;
        feathered_range::drag_range(&mut moved, &start, RangeHandle::Start, -0.05);
        // The edge moves with its fade.
        assert!((moved.range.inner_min - -0.3).abs() < 1e-6);
        assert!((moved.range.min - -0.5).abs() < 1e-6);
        assert_eq!(
            (moved.range.inner_max, moved.range.max),
            (start.range.inner_max, start.range.max)
        );

        let mut moved = start;
        feathered_range::drag_range(&mut moved, &start, RangeHandle::EndFeather, 0.025);
        assert!((moved.range.max - 0.45).abs() < 1e-6);
        assert_eq!(
            (
                moved.range.min,
                moved.range.inner_min,
                moved.range.inner_max
            ),
            (
                start.range.min,
                start.range.inner_min,
                start.range.inner_max
            )
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
