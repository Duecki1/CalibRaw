//! A range on a 0–1 track with a soft fade at each end, drawn as its weight
//! curve. The control works on four ordered points: where the selection
//! starts to fade in, where it is full, where it starts to fade out and where
//! it ends. Solid handles on the top line mark full strength; hollow handles
//! on the bottom line mark where each fade begins. Every handle moves only its
//! own point: a hollow handle stops at its solid one, and a solid handle
//! pushes only the handles it crosses. Models only convert the points to and
//! from what they store.

use std::ops::RangeInclusive;

use eframe::egui::{self, pos2, vec2, Pos2, Rect, Response, Sense, Stroke, Ui};
use moduwu_design::{NumberField, SliderMetrics};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RangeHandle {
    /// Where the start reaches full strength.
    Start,
    /// Where the end leaves full strength.
    End,
    /// Where the start fades in; moves only that point.
    StartFeather,
    /// Where the end has faded out; moves only that point.
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

/// The four handle positions on the 0–1 track, in order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RangePoints {
    pub(crate) fade_in: f32,
    pub(crate) full_from: f32,
    pub(crate) full_to: f32,
    pub(crate) fade_out: f32,
}

impl RangePoints {
    pub(crate) fn get(self, handle: RangeHandle) -> f32 {
        match handle {
            RangeHandle::StartFeather => self.fade_in,
            RangeHandle::Start => self.full_from,
            RangeHandle::End => self.full_to,
            RangeHandle::EndFeather => self.fade_out,
        }
    }

    /// The points on the track and in order, keeping each one's place where
    /// it can.
    fn clamped(self) -> Self {
        let fade_in = self.fade_in.clamp(0.0, 1.0);
        let full_from = self.full_from.clamp(fade_in, 1.0);
        let full_to = self.full_to.clamp(full_from, 1.0);
        Self {
            fade_in,
            full_from,
            full_to,
            fade_out: self.fade_out.clamp(full_to, 1.0),
        }
    }

    /// `handle` moved by `delta` from these points. A hollow (fade) handle
    /// moves alone and stops at its own solid handle, so the full-strength
    /// points never move with it. A solid handle moves its full-strength point
    /// and pushes any handle it crosses, including its own fade.
    pub(crate) fn dragged(self, handle: RangeHandle, delta: f32) -> Self {
        let p = self.clamped();
        let target = (p.get(handle) + delta).clamp(0.0, 1.0);
        let mut next = p;
        match handle {
            RangeHandle::StartFeather => next.fade_in = target.min(p.full_from),
            RangeHandle::EndFeather => next.fade_out = target.max(p.full_to),
            RangeHandle::Start => {
                next.full_from = target;
                next.fade_in = p.fade_in.min(target);
                next.full_to = p.full_to.max(target);
                next.fade_out = p.fade_out.max(next.full_to);
            }
            RangeHandle::End => {
                next.full_to = target;
                next.fade_out = p.fade_out.max(target);
                next.full_from = p.full_from.min(target);
                next.fade_in = p.fade_in.min(next.full_from);
            }
        }
        next
    }
}

/// A range shown on a 0–1 track.
pub(crate) trait FeatheredRange: Copy + PartialEq + Send + Sync + 'static {
    /// Whether the range ends with a fade-out; fog only fades in, so its end
    /// handles are hidden and fixed at 1.
    const HAS_END: bool = true;
    /// Where the handles are on the track.
    fn points(&self) -> RangePoints;
    /// The points this model can hold, given that `moved` was dragged to its
    /// place in `points`. A model with limits on a fade's width stops a
    /// dragged fade handle at them, and moves the fade handle along with a
    /// dragged solid one. Exact: storage rounding belongs in `set_points`.
    fn constrain(&self, points: RangePoints, _moved: RangeHandle) -> RangePoints {
        points
    }
    /// Stores `points`, which are ordered, on the track and constrained.
    fn set_points(&mut self, points: RangePoints);
    /// Selection weight (0–1) at track position `t`, drawn as the curve.
    fn weight(&self, t: f32) -> f32;
    /// Restores the default range (double-click).
    fn reset(&mut self);
}

fn handles<R: FeatheredRange>() -> impl Iterator<Item = RangeHandle> {
    RangeHandle::ALL
        .into_iter()
        .filter(|handle| R::HAS_END || !handle.is_end_side())
}

/// Track fraction moved by an arrow key, and with Shift held.
const KEY_STEP: f32 = 0.01;
const KEY_STEP_LARGE: f32 = 0.05;

/// Height of the curve area, from the slider metrics so touch layouts grow.
pub(crate) fn track_height(ui: &Ui) -> f32 {
    (SliderMetrics::of(ui.ctx()).header_height * 1.75).round()
}

/// The curve area inside the allocated rect, leaving room for handles.
pub(crate) fn track_rect(ui: &Ui, rect: Rect) -> Rect {
    rect.shrink(SliderMetrics::of(ui.ctx()).handle_radius + 2.0)
}

/// Where `handle` is drawn: solid handles on the full-strength (top) line,
/// hollow ones on the zero (bottom) line, so two handles at the same place can
/// still be told apart and grabbed.
pub(crate) fn handle_position<R: FeatheredRange>(
    track: Rect,
    range: &R,
    handle: RangeHandle,
) -> Pos2 {
    let t = range.points().clamped().get(handle);
    let y = if handle.is_feather() {
        track.bottom()
    } else {
        track.top()
    };
    pos2(egui::lerp(track.x_range(), t), y)
}

pub(crate) fn nearest_handle<R: FeatheredRange>(
    track: Rect,
    range: &R,
    pointer: Pos2,
) -> RangeHandle {
    // Resolve coincident handles by the side approached, so a collapsed range
    // can be opened in either direction.
    let points = range.points().clamped();
    let middle = egui::lerp(track.x_range(), (points.full_from + points.full_to) * 0.5);
    let prefer_end = R::HAS_END && pointer.x >= middle;
    handles::<R>()
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

/// A handle being dragged. It keeps the exact handle places from frame to
/// frame, so handles pushed along stay where they were left when the drag
/// reverses, and a model's storage rounding cannot make them drift.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HandleDrag {
    handle: RangeHandle,
    points: RangePoints,
}

impl HandleDrag {
    pub(crate) fn new<R: FeatheredRange>(range: &R, handle: RangeHandle) -> Self {
        Self {
            handle,
            points: range.points().clamped(),
        }
    }

    /// The dragged handle's track place.
    pub(crate) fn place(&self) -> f32 {
        self.points.get(self.handle)
    }

    /// Moves the handle to `target` (0–1) and stores the result in `range`.
    pub(crate) fn move_to<R: FeatheredRange>(&mut self, range: &mut R, target: f32) {
        let mut points = self
            .points
            .dragged(self.handle, target.clamp(0.0, 1.0) - self.place());
        if !R::HAS_END {
            points.full_to = 1.0;
            points.fade_out = 1.0;
        }
        self.points = range.constrain(points, self.handle);
        range.set_points(self.points);
    }
}

/// Moves `handle` by `delta` of the track from `start` in one step.
pub(crate) fn drag_range<R: FeatheredRange>(
    range: &mut R,
    start: &R,
    handle: RangeHandle,
    delta: f32,
) {
    let mut drag = HandleDrag::new(start, handle);
    *range = *start;
    drag.move_to(range, drag.place() + delta);
}

#[derive(Clone, Copy)]
struct RangeDrag {
    drag: HandleDrag,
    start_x: f32,
    /// The handle's track place when the drag began.
    start_value: f32,
}

/// Shows the curve. `background` paints the track behind it; `label` names
/// the control for accessibility and `value_text` reports its value.
pub(crate) fn feathered_range_track<R: FeatheredRange>(
    ui: &mut Ui,
    range: &mut R,
    label: &str,
    value_text: String,
    hover_text: &str,
    background: impl FnOnce(&egui::Painter, Rect),
) -> Response {
    let before = *range;
    let metrics = SliderMetrics::of(ui.ctx());
    let (rect, mut response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(1.0), track_height(ui)),
        Sense::click_and_drag(),
    );
    let track = track_rect(ui, rect);
    let enabled = ui.is_enabled();
    let drag_id = response.id.with("feathered-range-drag");
    let selected_id = response.id.with("feathered-range-selected");
    let mut selected = ui
        .data(|data| data.get_temp::<RangeHandle>(selected_id))
        .filter(|handle| handles::<R>().any(|available| available == *handle))
        .unwrap_or(RangeHandle::Start);

    // A press selects the nearest handle; the keyboard then edits it.
    if enabled && response.is_pointer_button_down_on() && ui.input(|i| i.pointer.any_pressed()) {
        if let Some(origin) = ui.input(|i| i.pointer.press_origin()) {
            selected = nearest_handle(track, range, origin);
            response.request_focus();
        }
    }
    if response.drag_started() {
        if let Some(origin) = ui.input(|i| i.pointer.press_origin()) {
            let horizontal = response.interact_pointer_pos().is_some_and(|pointer| {
                let delta = pointer - origin;
                delta.x.abs() >= delta.y.abs() * 1.15
            });
            if horizontal {
                selected = nearest_handle(track, range, origin);
                ui.data_mut(|data| {
                    data.insert_temp(
                        drag_id,
                        RangeDrag {
                            drag: HandleDrag::new(range, selected),
                            start_x: origin.x,
                            start_value: range.points().clamped().get(selected),
                        },
                    )
                });
            }
        }
    }
    let mut drag = ui.data(|data| data.get_temp::<RangeDrag>(drag_id));
    if response.dragged() {
        if let (Some(state), Some(pointer)) = (drag.as_mut(), response.interact_pointer_pos()) {
            moduwu_design::lock_slider_scroll(ui.ctx(), response.id);
            let moved = (pointer.x - state.start_x) / track.width().max(1.0);
            state.drag.move_to(range, state.start_value + moved);
            let state = *state;
            ui.data_mut(|data| data.insert_temp(drag_id, state));
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|data| data.remove::<RangeDrag>(drag_id));
    }
    if enabled && response.double_clicked() {
        range.reset();
    }

    if enabled && response.has_focus() {
        // Arrow keys edit the range instead of moving focus away.
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        let (left, right, up, down, large) = ui.input_mut(|input| {
            let large = input.modifiers.shift;
            let mut pressed = |key| {
                input.consume_key(egui::Modifiers::NONE, key)
                    || input.consume_key(egui::Modifiers::SHIFT, key)
            };
            (
                pressed(egui::Key::ArrowLeft),
                pressed(egui::Key::ArrowRight),
                pressed(egui::Key::ArrowUp),
                pressed(egui::Key::ArrowDown),
                large,
            )
        });
        let step = if large { KEY_STEP_LARGE } else { KEY_STEP };
        let delta = (f32::from(u8::from(right)) - f32::from(u8::from(left))) * step;
        if delta != 0.0 {
            let current = *range;
            drag_range(range, &current, selected, delta);
        }
        if up != down {
            let active: Vec<RangeHandle> = handles::<R>().collect();
            if let Some(index) = active.iter().position(|handle| *handle == selected) {
                let count = active.len();
                let next = if down { index + 1 } else { index + count - 1 };
                selected = active[next % count];
            }
        }
    }
    ui.data_mut(|data| data.insert_temp(selected_id, selected));
    if *range != before {
        response.mark_changed();
    }

    // The model's background in a framed area, then the weight curve.
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    background(&painter, track);
    let accent = if enabled {
        visuals.selection.bg_fill
    } else {
        visuals.widgets.noninteractive.fg_stroke.color
    };
    let curve_point = |t: f32| {
        pos2(
            egui::lerp(track.x_range(), t),
            track.bottom() - range.weight(t).clamp(0.0, 1.0) * track.height(),
        )
    };
    let mut previous = curve_point(0.0);
    for sample in 1..=128 {
        let next = curve_point(sample as f32 / 128.0);
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(previous.x, track.bottom()),
                previous,
                next,
                pos2(next.x, track.bottom()),
            ],
            accent.gamma_multiply(0.22),
            Stroke::NONE,
        ));
        painter.line_segment([previous, next], Stroke::new(2.0, accent));
        previous = next;
    }
    painter.rect_stroke(
        track,
        3.0,
        visuals.widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Outside,
    );

    // Handles styled like slider handles; the hollow ones change softness.
    let dragged = drag
        .filter(|_| response.dragged())
        .map(|state| state.drag.handle);
    let hovered = response
        .hover_pos()
        .filter(|_| enabled && dragged.is_none())
        .map(|pointer| nearest_handle(track, range, pointer));
    let foreground = visuals.widgets.active.fg_stroke.color;
    for handle in handles::<R>() {
        let center = handle_position(track, range, handle);
        let mut radius = if handle.is_feather() {
            metrics.handle_radius - 2.0
        } else {
            metrics.handle_radius
        };
        if dragged == Some(handle) || hovered == Some(handle) {
            radius += 1.0;
        }
        if handle.is_feather() {
            painter.circle_filled(center, radius, visuals.extreme_bg_color);
            painter.circle_stroke(center, radius, Stroke::new(2.0, accent));
        } else {
            painter.circle_filled(center, radius, accent);
            painter.circle_stroke(center, radius, Stroke::new(1.0, foreground));
        }
        if response.has_focus() && handle == selected {
            painter.circle_stroke(center, radius + 3.0, visuals.selection.stroke);
        }
    }

    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::WidgetType::Slider, enabled, label);
        info.current_text_value = Some(value_text.clone());
        info
    });
    response
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
        .on_hover_text(format!(
            "{hover_text}\nArrow keys move the selected handle (Shift for larger steps); Up and Down choose a handle. Double-click to reset."
        ))
}

/// A dark track with faint quarter marks, for ranges without a meaningful
/// backdrop.
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

/// A numeric entry shown under a range.
pub(crate) struct RangeField<'a> {
    pub(crate) label: &'a str,
    pub(crate) value: &'a mut f32,
    pub(crate) range: RangeInclusive<f32>,
    pub(crate) decimals: usize,
    pub(crate) speed: f64,
    /// Why the field is disabled, or `None` when it is enabled.
    pub(crate) disabled_reason: Option<&'a str>,
}

/// Numeric fields under a range, two per row in label–value pairs. They keep
/// every value reachable when handles overlap or a ramp runs off the track.
pub(crate) fn range_fields(ui: &mut Ui, mut fields: Vec<RangeField<'_>>) -> bool {
    let mut changed = false;
    let value_width = SliderMetrics::of(ui.ctx()).value_field_width;
    for pair in fields.chunks_mut(2) {
        ui.columns(2, |columns| {
            for (column, field) in columns.iter_mut().zip(pair.iter_mut()) {
                moduwu_design::property_row(column, field.label, |ui| {
                    let response = ui
                        .add_enabled_ui(field.disabled_reason.is_none(), |ui| {
                            ui.add_sized(
                                [value_width, moduwu_design::CONTROL_HEIGHT],
                                NumberField::new(&mut *field.value, field.range.clone())
                                    .speed(field.speed)
                                    .decimals(field.decimals),
                            )
                        })
                        .inner;
                    let response = match field.disabled_reason {
                        Some(reason) => response.on_disabled_hover_text(reason),
                        None => response,
                    };
                    changed |= response.changed();
                });
            }
        });
    }
    changed
}

/// `value` rounded to `decimals` places, so drags store what fields show.
pub(crate) fn round_to(value: f32, decimals: i32) -> f32 {
    let scale = 10_f32.powi(decimals);
    (value * scale).round() / scale
}
