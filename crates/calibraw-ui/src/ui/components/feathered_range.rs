//! A range on a 0–1 track with a soft fade at each end, drawn as its weight
//! curve. The control works on four ordered points: where the selection
//! starts to fade in, where it is full, where it starts to fade out and where
//! it ends. Solid handles on the top line mark full strength and move their
//! end with its feather unchanged; hollow handles on the bottom line mark
//! where each fade begins and change its feather. The handles are the only
//! editor: the keyboard and screen-reader actions move the selected one.
//! Models only convert the points to and from what they store.

use eframe::egui::{self, pos2, vec2, Pos2, Rect, Response, Sense, Stroke, Ui};
use moduwu_design::SliderMetrics;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RangeHandle {
    /// Where the start reaches full strength; moves the start with its fade.
    Start,
    /// Where the end leaves full strength; moves the end with its fade.
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

/// How one drag step changed an end of the range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EndChange {
    /// The full-strength point moved and carried its fade unchanged.
    Carried,
    /// Only the fade point moved.
    Faded,
    /// The full-strength point moved and its carried fade was squeezed so
    /// it stays on the track.
    Squeezed,
}

/// What a drag step did to one end, for its model to store. `full` is the
/// end's full-strength point and `fade` where its fade begins, as the drag
/// holds them (exact track places).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EndEdit {
    /// The full-strength point moved with its feather unchanged: the stored
    /// feather is kept exactly.
    Moved { full: f32 },
    /// Only the fade point moved: the full-strength point keeps its stored
    /// place and the feather changes.
    Faded { full: f32, fade: f32 },
    /// The full-strength point moved and its fade was squeezed at a track
    /// end: both are stored and the feather narrows.
    Squeezed { full: f32, fade: f32 },
}

impl EndEdit {
    /// The end's full-strength point and, when the feather changed, its fade
    /// point, in stored units. `stored_full` is the full-strength point as
    /// stored now; `store` converts a track place to stored units, rounding
    /// it to the stored precision. A point that did not move is not rounded.
    pub(crate) fn resolve(
        self,
        stored_full: f32,
        store: impl Fn(f32) -> f32,
    ) -> (f32, Option<f32>) {
        match self {
            Self::Moved { full } => (store(full), None),
            Self::Faded { fade, .. } => (stored_full, Some(store(fade))),
            Self::Squeezed { full, fade } => (store(full), Some(store(fade))),
        }
    }
}

/// The ends one drag step changed. An end that is `None` keeps its exact
/// stored values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RangeEdit {
    pub(crate) start: Option<EndEdit>,
    pub(crate) end: Option<EndEdit>,
}

impl RangeEdit {
    fn new(points: RangePoints, changes: [Option<EndChange>; 2]) -> Self {
        let edit = |change, full, fade| match change {
            EndChange::Carried => EndEdit::Moved { full },
            EndChange::Faded => EndEdit::Faded { full, fade },
            EndChange::Squeezed => EndEdit::Squeezed { full, fade },
        };
        Self {
            start: changes[0].map(|change| edit(change, points.full_from, points.fade_in)),
            end: changes[1].map(|change| edit(change, points.full_to, points.fade_out)),
        }
    }
}

/// A fade point carried from `was` to `moved` by its full-strength point,
/// stopped at the near track end, or where it already is if it was off the
/// track (older settings). Returns the place and how the end changed.
fn carry_left(was: f32, moved: f32) -> (f32, EndChange) {
    let floor = was.min(0.0);
    if moved < floor {
        (floor, EndChange::Squeezed)
    } else {
        (moved, EndChange::Carried)
    }
}

/// [`carry_left`] at the far track end.
fn carry_right(was: f32, moved: f32) -> (f32, EndChange) {
    let ceiling = was.max(1.0);
    if moved > ceiling {
        (ceiling, EndChange::Squeezed)
    } else {
        (moved, EndChange::Carried)
    }
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

    /// The points on the track, each fade on the outer side of its
    /// full-strength point. The two ends are not ordered against each other:
    /// a model may let their fades overlap (depth does), and such a range is
    /// edited as it is rather than silently reordered.
    fn clamped(self) -> Self {
        let fade_in = self.fade_in.clamp(0.0, 1.0);
        let full_to = self.full_to.clamp(0.0, 1.0);
        Self {
            fade_in,
            full_from: self.full_from.clamp(fade_in, 1.0),
            full_to,
            fade_out: self.fade_out.clamp(full_to, 1.0),
        }
    }

    /// The handles moved so that `handle` is at track place `target`, and
    /// how each end changed (`None`: untouched).
    ///
    /// | Handle                 | Moves                          | Feathers             |
    /// |------------------------|--------------------------------|----------------------|
    /// | `Start`, `End` (solid) | its end, and the other end it  | kept; squeezed only  |
    /// |                        | pushes in an ordered range     | at a track end       |
    /// | `StartFeather`,        | its fade point alone, up to    | sets its own         |
    /// | `EndFeather` (hollow)  | its solid handle               |                      |
    ///
    /// A solid handle reaches both track ends. It carries its fade, and a
    /// pushed end's fade, unchanged while they fit on the track; a fade that
    /// would leave the track stops at its end instead, squeezing that
    /// feather. A fade already off the track (older settings) stays where it
    /// is rather than being carried further off. Ends whose fades overlap
    /// (`full_from > full_to`, as depth allows) do not push each other.
    fn dragged(self, handle: RangeHandle, target: f32) -> (Self, [Option<EndChange>; 2]) {
        let target = target.clamp(0.0, 1.0);
        let ordered = self.full_from <= self.full_to;
        let mut next = self;
        let mut changes = [None, None];
        match handle {
            RangeHandle::StartFeather => {
                next.fade_in = target.min(self.full_from);
                changes[0] = Some(EndChange::Faded);
            }
            RangeHandle::EndFeather => {
                next.fade_out = target.max(self.full_to);
                changes[1] = Some(EndChange::Faded);
            }
            RangeHandle::Start => {
                let shift = target - self.full_from;
                next.full_from = target;
                let (fade_in, change) = carry_left(self.fade_in, self.fade_in + shift);
                next.fade_in = fade_in.min(target);
                changes[0] = Some(change);
                if ordered && target > self.full_to {
                    let push = target - self.full_to;
                    next.full_to = target;
                    let (fade_out, change) = carry_right(self.fade_out, self.fade_out + push);
                    next.fade_out = fade_out.max(target);
                    changes[1] = Some(change);
                }
            }
            RangeHandle::End => {
                let shift = target - self.full_to;
                next.full_to = target;
                let (fade_out, change) = carry_right(self.fade_out, self.fade_out + shift);
                next.fade_out = fade_out.max(target);
                changes[1] = Some(change);
                if ordered && target < self.full_from {
                    let push = target - self.full_from;
                    next.full_from = target;
                    let (fade_in, change) = carry_left(self.fade_in, self.fade_in + push);
                    next.fade_in = fade_in.min(target);
                    changes[0] = Some(change);
                }
            }
        }
        (next, changes)
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
    /// dragged fade handle at them. A constrained solid handle keeps its fade
    /// width unless the drag squeezed it. Exact: storage rounding belongs in
    /// `set_points`.
    fn constrain(&self, points: RangePoints, _moved: RangeHandle) -> RangePoints {
        points
    }
    /// Stores one drag step. The drag decides which ends changed and how
    /// ([`EndEdit`]); the model only converts to its stored values, for
    /// example with [`EndEdit::resolve`].
    fn set_points(&mut self, edit: RangeEdit);
    /// What `handle` sets and its stored value, such as "High feather 0.15",
    /// for screen readers.
    fn handle_text(&self, handle: RangeHandle) -> String;
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
/// hollow ones on the zero (bottom) line, so no two handles of different
/// kinds ever share a place and each stays grabbable. Where overlapping fades
/// keep the curve below full strength, a connector from the solid handle down
/// to the curve shows the weight actually reached.
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

/// The weight curve's height under a solid handle, when it is visibly below
/// full strength.
fn curve_below<R: FeatheredRange>(track: Rect, range: &R, handle: RangeHandle) -> Option<Pos2> {
    let t = range.points().clamped().get(handle);
    let weight = range.weight(t);
    (weight.is_finite() && weight < 0.99).then(|| {
        pos2(
            egui::lerp(track.x_range(), t),
            track.bottom() - weight.clamp(0.0, 1.0) * track.height(),
        )
    })
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
            points: range.points(),
        }
    }

    /// The dragged handle's place as drawn (fades off the track are drawn at
    /// its end).
    pub(crate) fn place(&self) -> f32 {
        self.points.clamped().get(self.handle)
    }

    /// Moves the handle to `target` (0–1) and stores the result in `range`.
    pub(crate) fn move_to<R: FeatheredRange>(&mut self, range: &mut R, target: f32) {
        let (mut points, mut changes) = self.points.dragged(self.handle, target);
        if !R::HAS_END {
            points.full_to = 1.0;
            points.fade_out = 1.0;
            changes[1] = None;
        }
        self.points = range.constrain(points, self.handle);
        range.set_points(RangeEdit::new(self.points, changes));
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
/// the control for accessibility. The control is one focus stop: its value
/// is the selected handle's, which the arrow keys and the screen-reader
/// increment, decrement and set-value actions move.
pub(crate) fn feathered_range_track<R: FeatheredRange>(
    ui: &mut Ui,
    range: &mut R,
    label: &str,
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
    if enabled {
        // Screen-reader actions move the selected handle like the arrow keys.
        let (steps, set_to) = ui.input(|input| {
            use egui::accesskit::{Action, ActionData};
            let steps = input.num_accesskit_action_requests(response.id, Action::Increment) as f32
                - input.num_accesskit_action_requests(response.id, Action::Decrement) as f32;
            let set_to = input
                .accesskit_action_requests(response.id, Action::SetValue)
                .filter_map(|request| match request.data {
                    Some(ActionData::NumericValue(value)) => Some(value as f32),
                    _ => None,
                })
                .last();
            (steps, set_to)
        });
        if steps != 0.0 {
            let current = *range;
            drag_range(range, &current, selected, steps * KEY_STEP);
        }
        if let Some(target) = set_to.filter(|target| target.is_finite()) {
            let current = *range;
            let place = current.points().clamped().get(selected);
            drag_range(range, &current, selected, target - place);
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
    for handle in handles::<R>().filter(|handle| !handle.is_feather()) {
        if let Some(on_curve) = curve_below(track, range, handle) {
            let center = handle_position(track, range, handle);
            painter.line_segment(
                [center, on_curve],
                Stroke::new(1.0, accent.gamma_multiply(0.7)),
            );
            painter.circle_filled(on_curve, 2.5, accent);
        }
    }
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

    // The selected handle is the slider's value; every handle is described.
    let place = range.points().clamped().get(selected);
    let selected_text = range.handle_text(selected);
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::WidgetType::Slider, enabled, label);
        info.value = Some(f64::from(place));
        info.current_text_value = Some(selected_text.clone());
        info
    });
    let description = handles::<R>()
        .map(|handle| range.handle_text(handle))
        .collect::<Vec<_>>()
        .join(", ");
    ui.ctx().accesskit_node_builder(response.id, |builder| {
        use egui::accesskit::Action;
        builder.set_description(description);
        builder.set_min_numeric_value(0.0);
        builder.set_max_numeric_value(1.0);
        builder.set_numeric_value_step(f64::from(KEY_STEP));
        if enabled {
            builder.add_action(Action::SetValue);
            if place < 1.0 {
                builder.add_action(Action::Increment);
            }
            if place > 0.0 {
                builder.add_action(Action::Decrement);
            }
        }
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

/// `value` rounded to `decimals` places, the precision models store.
pub(crate) fn round_to(value: f32, decimals: i32) -> f32 {
    let scale = 10_f32.powi(decimals);
    (value * scale).round() / scale
}
