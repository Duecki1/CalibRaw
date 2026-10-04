use crate::app::ToneCurveTab;
use crate::pipeline::{PointCurve, MAX_POINT_CURVE_POINTS};
use crate::ui::theme::{CHANNEL_BLUE, CHANNEL_GREEN, CHANNEL_RED};
#[cfg(not(target_os = "android"))]
use eframe::egui::Align;
use eframe::egui::{self, Color32, Pos2, Sense, Stroke, StrokeKind, Ui};

const CURVE_HEIGHT: f32 = 210.0;
const POINT_RADIUS: f32 = 5.0;
const PICK_RADIUS: f32 = 16.0;
const MIN_POINT_X_GAP: f32 = 0.005;

/// The curve point being dragged and its offset from the pointer, so it moves
/// with the pointer instead of jumping onto it.
#[derive(Clone, Copy)]
struct GrabbedPoint {
    index: usize,
    offset: [f32; 2],
}

pub(crate) fn tone_curve_editor(ui: &mut Ui, curve: &mut PointCurve, curve_color: Color32) -> bool {
    curve.sanitize();
    let width = ui.available_width().max(1.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, CURVE_HEIGHT), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let overlay_painter = ui.painter().with_clip_rect(ui.clip_rect());
    let visuals = ui.visuals();

    painter.rect_filled(rect, 4.0, visuals.extreme_bg_color);
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
        StrokeKind::Inside,
    );

    for step in 1..4 {
        let t = step as f32 / 4.0;
        let x = egui::lerp(rect.left()..=rect.right(), t);
        let y = egui::lerp(rect.bottom()..=rect.top(), t);
        let grid = Stroke::new(1.0, visuals.faint_bg_color);
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            grid,
        );
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            grid,
        );
    }

    painter.line_segment(
        [
            Pos2::new(rect.left(), rect.bottom()),
            Pos2::new(rect.right(), rect.top()),
        ],
        Stroke::new(1.0, visuals.weak_text_color()),
    );

    let mut previous = curve_to_screen(rect, [0.0, sample_curve(curve, 0.0)]);
    for sample in 1..=128 {
        let x = sample as f32 / 128.0;
        let next = curve_to_screen(rect, [x, sample_curve(curve, x)]);
        overlay_painter.line_segment([previous, next], Stroke::new(2.0, curve_color));
        previous = next;
    }

    for point in curve.points.iter().take(curve.len as usize) {
        let center = curve_to_screen(rect, *point);
        overlay_painter.circle_filled(center, POINT_RADIUS, Color32::WHITE);
        overlay_painter.circle_stroke(center, POINT_RADIUS, Stroke::new(1.5, curve_color));
    }

    let mut changed = false;
    // The point is chosen where the press began and kept for the whole drag.
    // Re-picking the nearest point each frame dropped it whenever the pointer
    // outran it (e.g. while it was pinned at an edge or a neighbor) or
    // switched to a neighbor passed on the way.
    let drag_id = response.id.with("dragged-point");
    if response.drag_started() {
        let grabbed = ui
            .input(|input| input.pointer.press_origin())
            .and_then(|origin| {
                let index = nearest_point(curve, rect, origin, PICK_RADIUS * 2.0)?;
                let pointer = screen_to_curve(rect, origin);
                let point = curve.points[index];
                Some(GrabbedPoint {
                    index,
                    offset: [point[0] - pointer[0], point[1] - pointer[1]],
                })
            });
        ui.data_mut(|data| match grabbed {
            Some(grabbed) => {
                data.insert_temp(drag_id, grabbed);
            }
            None => {
                data.remove::<GrabbedPoint>(drag_id);
            }
        });
    }
    let grabbed = response
        .dragged()
        .then(|| ui.data(|data| data.get_temp::<GrabbedPoint>(drag_id)))
        .flatten()
        .filter(|grabbed| grabbed.index < curve.len as usize);
    if let (Some(pointer), Some(GrabbedPoint { index, offset })) =
        (response.interact_pointer_pos(), grabbed)
    {
        let pointer = screen_to_curve(rect, pointer);
        let mut normalized = [pointer[0] + offset[0], pointer[1] + offset[1]];
        let len = curve.len as usize;
        let min_x = if index == 0 {
            0.0
        } else {
            curve.points[index - 1][0] + MIN_POINT_X_GAP
        };
        let max_x = if index + 1 == len {
            1.0
        } else {
            curve.points[index + 1][0] - MIN_POINT_X_GAP
        };
        normalized[0] = normalized[0].clamp(min_x, max_x.max(min_x));
        normalized[1] = normalized[1].clamp(0.0, 1.0);
        if curve.points[index] != normalized {
            curve.points[index] = normalized;
            changed = true;
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|data| data.remove::<GrabbedPoint>(drag_id));
    }

    #[cfg(not(target_os = "android"))]
    if response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let point = screen_to_curve(rect, pointer);
            changed |= insert_point(curve, point);
        }
    }

    #[cfg(target_os = "android")]
    if response.double_clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let point = screen_to_curve(rect, pointer);
            changed |= insert_point(curve, point);
        }
    }

    if response.secondary_clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            if let Some(index) = nearest_point(curve, rect, pointer, PICK_RADIUS) {
                changed |= remove_point(curve, index);
            }
        }
    }

    #[cfg(not(target_os = "android"))]
    response.on_hover_text(
        "Click to add a point. Drag points to shape the curve; right-click an interior point to remove it.",
    );
    #[cfg(target_os = "android")]
    response.on_hover_text("Drag points to shape the curve. Double-tap to add a point.");
    curve.sanitize();
    changed
}

fn curve_to_screen(rect: egui::Rect, point: [f32; 2]) -> Pos2 {
    Pos2::new(
        egui::lerp(rect.left()..=rect.right(), point[0]),
        egui::lerp(rect.bottom()..=rect.top(), point[1]),
    )
}

fn screen_to_curve(rect: egui::Rect, point: Pos2) -> [f32; 2] {
    [
        ((point.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
        ((rect.bottom() - point.y) / rect.height()).clamp(0.0, 1.0),
    ]
}

fn nearest_point(
    curve: &PointCurve,
    rect: egui::Rect,
    pointer: Pos2,
    radius: f32,
) -> Option<usize> {
    curve
        .points
        .iter()
        .take(curve.len as usize)
        .enumerate()
        .map(|(index, point)| (index, curve_to_screen(rect, *point).distance(pointer)))
        .filter(|(_, distance)| *distance <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

fn insert_point(curve: &mut PointCurve, point: [f32; 2]) -> bool {
    let len = curve.len as usize;
    if len >= MAX_POINT_CURVE_POINTS
        || point[0] <= curve.points[0][0] + MIN_POINT_X_GAP
        || point[0] >= curve.points[len - 1][0] - MIN_POINT_X_GAP
    {
        return false;
    }
    if curve.points[..len]
        .iter()
        .any(|existing| (existing[0] - point[0]).abs() < 0.015)
    {
        return false;
    }

    let insert_at = curve.points[..len]
        .iter()
        .position(|existing| existing[0] > point[0])
        .unwrap_or(len - 1);
    for index in (insert_at..len).rev() {
        curve.points[index + 1] = curve.points[index];
    }
    curve.points[insert_at] = [point[0], point[1].clamp(0.0, 1.0)];
    curve.len += 1;
    true
}

fn remove_point(curve: &mut PointCurve, index: usize) -> bool {
    let len = curve.len as usize;
    if index == 0 || index + 1 == len || len <= 2 {
        return false;
    }
    for current in index..len - 1 {
        curve.points[current] = curve.points[current + 1];
    }
    curve.points[len - 1] = [1.0, 1.0];
    curve.len -= 1;
    true
}

fn sample_curve(curve: &PointCurve, input: f32) -> f32 {
    let len = curve.len.clamp(2, MAX_POINT_CURVE_POINTS as u32) as usize;
    let x = input.clamp(0.0, 1.0);
    let mut segment = len - 2;
    for index in 0..len - 1 {
        if x <= curve.points[index + 1][0] {
            segment = index;
            break;
        }
    }

    let p0 = curve.points[segment];
    let p1 = curve.points[segment + 1];
    let width = (p1[0] - p0[0]).max(1e-5);
    let t = ((x - p0[0]) / width).clamp(0.0, 1.0);
    let t2 = t * t;
    let t3 = t2 * t;
    let m0 = tangent(curve, segment, len) * width;
    let m1 = tangent(curve, segment + 1, len) * width;
    let y = (2.0 * t3 - 3.0 * t2 + 1.0) * p0[1]
        + (t3 - 2.0 * t2 + t) * m0
        + (-2.0 * t3 + 3.0 * t2) * p1[1]
        + (t3 - t2) * m1;
    y.clamp(p0[1].min(p1[1]), p0[1].max(p1[1]))
}

fn tangent(curve: &PointCurve, index: usize, len: usize) -> f32 {
    if index == 0 {
        return secant(curve.points[0], curve.points[1]);
    }
    if index + 1 >= len {
        return secant(curve.points[len - 2], curve.points[len - 1]);
    }
    let previous = secant(curve.points[index - 1], curve.points[index]);
    let next = secant(curve.points[index], curve.points[index + 1]);
    if previous * next <= 0.0 {
        0.0
    } else {
        2.0 * previous * next / (previous + next)
    }
}

fn secant(a: [f32; 2], b: [f32; 2]) -> f32 {
    (b[1] - a[1]) / (b[0] - a[0]).max(1e-5)
}

pub(crate) struct ToneCurveChannels<'a> {
    pub rgb: &'a mut PointCurve,
    pub red: &'a mut PointCurve,
    pub green: &'a mut PointCurve,
    pub blue: &'a mut PointCurve,
}

const TONE_CURVE_TABS: [(ToneCurveTab, &str, Color32); 4] = [
    (ToneCurveTab::Rgb, "RGB", Color32::WHITE),
    (ToneCurveTab::Red, "R", CHANNEL_RED),
    (ToneCurveTab::Green, "G", CHANNEL_GREEN),
    (ToneCurveTab::Blue, "B", CHANNEL_BLUE),
];

pub(crate) fn tone_curve_channel_editor(
    ui: &mut Ui,
    mut curves: ToneCurveChannels<'_>,
    selected_tab: &mut ToneCurveTab,
    min_segment_width: f32,
) -> bool {
    let mut changed = false;
    #[cfg(target_os = "android")]
    let _ = min_segment_width;

    #[cfg(not(target_os = "android"))]
    ui.horizontal(|ui| {
        let spacing = ui.spacing().item_spacing.x;
        let segment_width =
            ((ui.available_width() - moduwu_design::TOOLBAR_ICON_EDGE - spacing * 4.0)
                .max(min_segment_width))
                / 4.0;
        for (tab, label, color) in TONE_CURVE_TABS {
            let text = egui::RichText::new(label).color(color);
            if moduwu_design::segmented_button(ui, text, *selected_tab == tab, segment_width)
                .on_hover_text(tone_curve_description(tab))
                .clicked()
            {
                *selected_tab = tab;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            if moduwu_design::icon_button(
                ui,
                egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                moduwu_design::toolbar_icon_size(),
                "Reset the selected tone curve",
            )
            .clicked()
            {
                reset_selected_tone_curve(&mut curves, *selected_tab);
                changed = true;
            }
        });
    });

    #[cfg(not(target_os = "android"))]
    {
        let (curve, color) = selected_tone_curve(curves, *selected_tab);
        changed |= tone_curve_editor(ui, curve, color);
    }

    #[cfg(target_os = "android")]
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            let control_height = moduwu_design::CONTROL_HEIGHT;
            ui.spacing_mut().item_spacing.y =
                ((CURVE_HEIGHT - control_height * 5.0) / 4.0).max(0.0);

            for (tab, label, color) in TONE_CURVE_TABS {
                let text = egui::RichText::new(label).color(color);
                if moduwu_design::segmented_button(
                    ui,
                    text,
                    *selected_tab == tab,
                    moduwu_design::TOOLBAR_ICON_EDGE,
                )
                .on_hover_text(tone_curve_description(tab))
                .clicked()
                {
                    *selected_tab = tab;
                }
            }

            if moduwu_design::icon_button(
                ui,
                egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                moduwu_design::toolbar_icon_size(),
                "Reset the selected tone curve",
            )
            .clicked()
            {
                reset_selected_tone_curve(&mut curves, *selected_tab);
                changed = true;
            }
        });

        let (curve, color) = selected_tone_curve(curves, *selected_tab);
        changed |= tone_curve_editor(ui, curve, color);
    });

    changed
}

fn selected_tone_curve(
    curves: ToneCurveChannels<'_>,
    selected_tab: ToneCurveTab,
) -> (&mut PointCurve, Color32) {
    match selected_tab {
        ToneCurveTab::Rgb => (curves.rgb, Color32::WHITE),
        ToneCurveTab::Red => (curves.red, CHANNEL_RED),
        ToneCurveTab::Green => (curves.green, CHANNEL_GREEN),
        ToneCurveTab::Blue => (curves.blue, CHANNEL_BLUE),
    }
}

fn reset_selected_tone_curve(curves: &mut ToneCurveChannels<'_>, selected_tab: ToneCurveTab) {
    match selected_tab {
        ToneCurveTab::Rgb => curves.rgb.reset(),
        ToneCurveTab::Red => curves.red.reset(),
        ToneCurveTab::Green => curves.green.reset(),
        ToneCurveTab::Blue => curves.blue.reset(),
    }
}

fn tone_curve_description(tab: ToneCurveTab) -> &'static str {
    match tab {
        ToneCurveTab::Rgb => "Composite luminance curve",
        ToneCurveTab::Red => "Red channel curve",
        ToneCurveTab::Green => "Green channel curve",
        ToneCurveTab::Blue => "Blue channel curve",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(ctx: &egui::Context, curve: &mut PointCurve, events: Vec<egui::Event>) -> egui::Rect {
        let mut rect = egui::Rect::NOTHING;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(320.0, 260.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                ui.set_width(300.0);
                tone_curve_editor(ui, curve, Color32::WHITE);
                rect = ui.min_rect();
            },
        );
        rect
    }

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn dragged_point_stays_grabbed_past_its_neighbor_and_the_edge() {
        let ctx = egui::Context::default();
        let mut curve = PointCurve::linear();
        assert!(insert_point(&mut curve, [0.5, 0.5]));
        assert!(insert_point(&mut curve, [0.6, 0.6]));
        let rect = show(&ctx, &mut curve, vec![]);
        let rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), CURVE_HEIGHT));
        let start = curve_to_screen(rect, curve.points[1]);
        show(
            &ctx,
            &mut curve,
            vec![egui::Event::PointerMoved(start), button(start, true)],
        );
        // Sweep right over the neighbor and far past the top edge; the
        // grabbed point stays pinned against both instead of being dropped.
        for step in 1..=6 {
            let pointer = start + egui::vec2(step as f32 * 20.0, -step as f32 * 60.0);
            show(&ctx, &mut curve, vec![egui::Event::PointerMoved(pointer)]);
        }
        assert_eq!(curve.len, 4);
        assert!((curve.points[1][0] - (0.6 - MIN_POINT_X_GAP)).abs() < 1e-4);
        assert_eq!(curve.points[1][1], 1.0);
        assert!((curve.points[2][0] - 0.6).abs() < 1e-6);
        assert!((curve.points[2][1] - 0.6).abs() < 1e-6);
    }
}
