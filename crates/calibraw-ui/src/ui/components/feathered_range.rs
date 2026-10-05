//! A range on a 0–1 track with a feathered ramp at each end, edited with four
//! handles: a circle at each ramp's centre and a diamond where each ramp
//! reaches full strength. The model decides where handles sit, how they move
//! and what weight the curve shows; this control owns picking, dragging,
//! scroll-gesture separation, reset and painting.

use eframe::egui::{self, pos2, vec2, Pos2, Rect, Response, Sense, Stroke, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RangeHandle {
    Start,
    End,
    StartFeather,
    EndFeather,
}

impl RangeHandle {
    pub(crate) const ALL: [Self; 4] =
        [Self::Start, Self::End, Self::StartFeather, Self::EndFeather];

    fn is_end_side(self) -> bool {
        matches!(self, Self::End | Self::EndFeather)
    }

    fn is_feather(self) -> bool {
        matches!(self, Self::StartFeather | Self::EndFeather)
    }
}

/// A range shown on a 0–1 track. Handle values are track positions.
pub(crate) trait FeatheredRange: Copy + PartialEq + Send + Sync + 'static {
    /// The handle's place on the track (0–1).
    fn handle_value(&self, handle: RangeHandle) -> f32;
    /// Hidden handles cannot be grabbed, e.g. a feather with no room to ramp.
    fn handle_active(&self, handle: RangeHandle) -> bool;
    /// Moves `handle` to its place in `start` plus `delta` of the track, so it
    /// stays under the pointer instead of first absorbing an off-track excess.
    fn drag(&mut self, start: &Self, handle: RangeHandle, delta: f32);
    /// Selection weight (0–1) at track position `t`, drawn as the curve.
    fn weight(&self, t: f32) -> f32;
    /// Restores the default range (double-click).
    fn reset(&mut self);
}

pub(crate) fn handle_position<R: FeatheredRange>(
    track: Rect,
    range: &R,
    handle: RangeHandle,
) -> Pos2 {
    let y = if handle.is_feather() {
        track.top()
    } else {
        track.bottom()
    };
    pos2(
        egui::lerp(track.x_range(), handle.handle_value_clamped(range)),
        y,
    )
}

impl RangeHandle {
    fn handle_value_clamped<R: FeatheredRange>(self, range: &R) -> f32 {
        range.handle_value(self).clamp(0.0, 1.0)
    }
}

pub(crate) fn nearest_handle<R: FeatheredRange>(
    track: Rect,
    range: &R,
    pointer: Pos2,
) -> RangeHandle {
    // Resolve coincident handles by the side approached, so a collapsed range
    // can be opened in either direction (including at 0 and 1).
    let start = range.handle_value(RangeHandle::Start);
    let end = range.handle_value(RangeHandle::End);
    let prefer_end = pointer.x >= egui::lerp(track.x_range(), (start + end) * 0.5) && end < 1.0;
    RangeHandle::ALL
        .into_iter()
        .filter(|handle| range.handle_active(*handle))
        .min_by(|a, b| {
            let a_pos = handle_position(track, range, *a);
            let b_pos = handle_position(track, range, *b);
            pointer
                .distance_sq(a_pos)
                .total_cmp(&pointer.distance_sq(b_pos))
                .then_with(|| (a.is_end_side() != prefer_end).cmp(&(b.is_end_side() != prefer_end)))
        })
        .unwrap_or(RangeHandle::Start)
}

#[derive(Clone, Copy)]
struct RangeDrag<R> {
    handle: RangeHandle,
    start_x: f32,
    initial: R,
}

pub(crate) const TRACK_HEIGHT: f32 = 82.0;
/// Inset of the track inside the allocated rect, leaving room for handles.
pub(crate) const TRACK_INSET: egui::Vec2 = vec2(10.0, 14.0);

/// Shows the track. `background` paints behind the weight curve; `label`
/// names the control for accessibility and `value_text` reports its value.
pub(crate) fn feathered_range_track<R: FeatheredRange>(
    ui: &mut Ui,
    range: &mut R,
    label: &str,
    value_text: String,
    hover_text: &str,
    background: impl FnOnce(&egui::Painter, Rect),
) -> Response {
    let before = *range;
    let (rect, mut response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(1.0), TRACK_HEIGHT),
        Sense::click_and_drag(),
    );
    let track = rect.shrink2(TRACK_INSET);
    let drag_id = response.id.with("feathered-range-drag");
    if response.drag_started() {
        if let Some(origin) = ui.input(|i| i.pointer.press_origin()) {
            let horizontal = response.interact_pointer_pos().is_some_and(|pointer| {
                let delta = pointer - origin;
                delta.x.abs() >= delta.y.abs() * 1.15
            });
            if horizontal {
                ui.data_mut(|data| {
                    data.insert_temp(
                        drag_id,
                        RangeDrag {
                            handle: nearest_handle(track, range, origin),
                            start_x: origin.x,
                            initial: *range,
                        },
                    )
                });
            }
        }
    }
    if response.dragged() {
        if let (Some(drag), Some(pointer)) = (
            ui.data(|data| data.get_temp::<RangeDrag<R>>(drag_id)),
            response.interact_pointer_pos(),
        ) {
            moduwu_design::lock_slider_scroll(ui.ctx(), response.id);
            range.drag(
                &drag.initial,
                drag.handle,
                (pointer.x - drag.start_x) / track.width().max(1.0),
            );
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|data| data.remove::<RangeDrag<R>>(drag_id));
    }
    if response.double_clicked() {
        range.reset();
    }
    if *range != before {
        response.mark_changed();
    }

    let painter = ui.painter_at(rect);
    background(&painter, track);
    let accent = ui.visuals().selection.bg_fill;
    let mut previous = pos2(
        track.left(),
        track.bottom() - range.weight(0.0) * track.height(),
    );
    for sample in 1..=128 {
        let t = sample as f32 / 128.0;
        let next = pos2(
            egui::lerp(track.x_range(), t),
            track.bottom() - range.weight(t) * track.height(),
        );
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(previous.x, track.bottom()),
                previous,
                next,
                pos2(next.x, track.bottom()),
            ],
            accent.gamma_multiply(0.25),
            Stroke::NONE,
        ));
        painter.line_segment([previous, next], Stroke::new(2.0, accent));
        previous = next;
    }
    for handle in RangeHandle::ALL
        .into_iter()
        .filter(|handle| range.handle_active(*handle))
    {
        let center = handle_position(track, range, handle);
        let stroke = Stroke::new(1.5, ui.visuals().text_color());
        if handle.is_feather() {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    center + vec2(0.0, -6.0),
                    center + vec2(6.0, 0.0),
                    center + vec2(0.0, 6.0),
                    center + vec2(-6.0, 0.0),
                ],
                ui.visuals().extreme_bg_color,
                stroke,
            ));
        } else {
            painter.line_segment(
                [pos2(center.x, track.top()), center],
                Stroke::new(1.0, accent),
            );
            painter.circle(center, 6.0, accent, stroke);
        }
    }
    let enabled = ui.is_enabled();
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::WidgetType::Other, enabled, label);
        info.current_text_value = Some(value_text.clone());
        info
    });
    response.on_hover_text(hover_text)
}

/// A dark track with quarter marks, for ranges without a meaningful backdrop.
pub(crate) fn plain_track_background(
    visuals: &egui::Visuals,
) -> impl FnOnce(&egui::Painter, Rect) + use<> {
    let (fill, marks) = (visuals.extreme_bg_color, visuals.faint_bg_color);
    move |painter, track| {
        painter.rect_filled(track, 3.0, fill);
        for step in 1..4 {
            let x = egui::lerp(track.x_range(), step as f32 / 4.0);
            painter.line_segment(
                [pos2(x, track.top()), pos2(x, track.bottom())],
                Stroke::new(1.0, marks),
            );
        }
    }
}
