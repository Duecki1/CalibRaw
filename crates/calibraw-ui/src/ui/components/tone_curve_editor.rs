use crate::app::ToneCurveTab;
use crate::i18n::tr;
use crate::pipeline::{PointCurve, MAX_POINT_CURVE_POINTS};
use crate::ui::theme::{CHANNEL_BLUE, CHANNEL_GREEN, CHANNEL_RED};
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

/// The curve editor. With `touch`, a drag that begins away from every point
/// is left to the scroll area around the editor, so the panel scrolls.
pub(crate) fn tone_curve_editor(
    ui: &mut Ui,
    curve: &mut PointCurve,
    curve_color: Color32,
    touch: bool,
) -> bool {
    curve.sanitize();
    let width = ui.available_width().max(1.0);
    let area_sense = if touch {
        Sense::click()
    } else {
        Sense::click_and_drag()
    };
    let (rect, area) = ui.allocate_exact_size(egui::vec2(width, CURVE_HEIGHT), area_sense);
    // Each point takes drags that begin within the radius a drag picks it
    // from. Its taps count as taps on the curve: a widget that takes drags
    // also takes the clicks on it, so the area beneath would miss them.
    let area_id = area.id;
    let response = (0..curve.len as usize).fold(area, |response, index| {
        let hit = egui::Rect::from_center_size(
            curve_to_screen(rect, curve.points[index]),
            egui::Vec2::splat(PICK_RADIUS * 4.0),
        )
        .intersect(rect);
        response.union(ui.interact(
            hit,
            area_id.with(("curve-point", index)),
            Sense::click_and_drag(),
        ))
    });
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

    // Touch adds points with a double tap, so a single tap can select one.
    if !touch && response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let point = screen_to_curve(rect, pointer);
            changed |= insert_point(curve, point);
        }
    }

    if touch && moduwu_design::double_tapped(&response) {
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

/// The channel tabs with their labels and colours.
fn tone_curve_tabs() -> [(ToneCurveTab, &'static str, Color32); 4] {
    [
        (ToneCurveTab::Rgb, tr("RGB"), Color32::WHITE),
        (ToneCurveTab::Red, "R", CHANNEL_RED),
        (ToneCurveTab::Green, "G", CHANNEL_GREEN),
        (ToneCurveTab::Blue, "B", CHANNEL_BLUE),
    ]
}

pub(crate) fn tone_curve_channel_editor(
    ui: &mut Ui,
    mut curves: ToneCurveChannels<'_>,
    selected_tab: &mut ToneCurveTab,
    min_segment_width: f32,
) -> bool {
    let mut changed = false;
    // Touch puts the channel tabs in a rail beside the curve.
    let touch = crate::platform::PlatformCaps::CURRENT.is_mobile();

    if !touch {
        ui.horizontal(|ui| {
            let spacing = ui.spacing().item_spacing.x;
            let segment_width =
                ((ui.available_width() - moduwu_design::TOOLBAR_ICON_EDGE - spacing * 4.0)
                    .max(min_segment_width))
                    / 4.0;
            for (tab, label, color) in tone_curve_tabs() {
                let text = egui::RichText::new(label).color(color);
                if moduwu_design::segmented_button(ui, text, *selected_tab == tab, segment_width)
                    .on_hover_text(tone_curve_description(tab))
                    .clicked()
                {
                    *selected_tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if moduwu_design::ghost_icon_button(
                    ui,
                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                    moduwu_design::toolbar_icon_size(),
                    tr("Reset the selected tone curve"),
                )
                .clicked()
                {
                    reset_selected_tone_curve(&mut curves, *selected_tab);
                    changed = true;
                }
            });
        });

        {
            let (curve, color) = selected_tone_curve(curves, *selected_tab);
            changed |= tone_curve_editor(ui, curve, color, touch);
        }
    } else {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                let control_height = moduwu_design::CONTROL_HEIGHT;
                ui.spacing_mut().item_spacing.y =
                    ((CURVE_HEIGHT - control_height * 5.0) / 4.0).max(0.0);

                for (tab, label, color) in tone_curve_tabs() {
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

                if moduwu_design::ghost_icon_button(
                    ui,
                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                    moduwu_design::toolbar_icon_size(),
                    tr("Reset the selected tone curve"),
                )
                .clicked()
                {
                    reset_selected_tone_curve(&mut curves, *selected_tab);
                    changed = true;
                }
            });

            let (curve, color) = selected_tone_curve(curves, *selected_tab);
            changed |= tone_curve_editor(ui, curve, color, touch);
        });
    }

    // Touch has no hover, so the gestures are stated as visible text.
    if touch {
        ui.add(
            egui::Label::new(
                egui::RichText::new(tr(
                    "Double-tap to add a point. Long-press a point to remove it.",
                ))
                .small()
                .color(ui.visuals().weak_text_color()),
            )
            .wrap(),
        );
    }

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
        ToneCurveTab::Rgb => tr("Composite luminance curve"),
        ToneCurveTab::Red => tr("Red channel curve"),
        ToneCurveTab::Green => tr("Green channel curve"),
        ToneCurveTab::Blue => tr("Blue channel curve"),
    }
}
