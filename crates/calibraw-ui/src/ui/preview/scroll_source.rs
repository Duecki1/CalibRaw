//! Tells a touchpad's two-finger scroll from a mouse wheel, so the preview
//! can pan with the one and zoom with the other.
//!
//! winit reports both as wheel events. A touchpad sends pixel deltas with
//! start and end phases (Wayland, macOS) or fractional line deltas without
//! phases (Windows, X11); a mouse wheel sends whole line steps. Each scroll
//! gesture is classified by its first event and keeps that class until it
//! ends, so one gesture never switches between panning and zooming.

use eframe::egui::{self, Event, MouseWheelUnit, TouchPhase, Vec2};

/// Seconds without a wheel event that end a gesture which reports no end
/// phase; egui's wheel state uses the same threshold.
const GESTURE_IDLE_SECONDS: f64 = 0.15;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ScrollSource {
    Wheel,
    Touchpad,
}

/// The scroll gesture in progress, kept in egui memory between frames.
#[derive(Clone, Copy, Default)]
struct ScrollGesture {
    source: Option<ScrollSource>,
    /// `InputState::time` of the gesture's latest wheel event, in seconds.
    last_event: f64,
}

/// This frame's scroll, by what produced it.
pub(super) struct FrameScroll {
    /// The device of the gesture in progress, if any.
    pub(super) source: Option<ScrollSource>,
    /// The touchpad's unsmoothed movement this frame in points, in egui's
    /// convention (the direction the content moves). Excludes scrolls the
    /// zoom modifier turns into zoom, which `zoom_delta` reports.
    pub(super) touchpad_pan: Vec2,
}

/// Classifies this frame's wheel events. Call once per frame: each call
/// advances the gesture state.
pub(super) fn frame_scroll(ctx: &egui::Context) -> FrameScroll {
    let id = egui::Id::new("calibraw-preview-scroll-gesture");
    let mut gesture: ScrollGesture = ctx.data(|data| data.get_temp(id)).unwrap_or_default();
    let (line_points, zoom_modifier) = ctx.options(|options| {
        (
            options.input_options.line_scroll_speed,
            options.input_options.zoom_modifier,
        )
    });
    let touchpad_pan = ctx.input(|input| {
        if input.time - gesture.last_event > GESTURE_IDLE_SECONDS {
            gesture.source = None;
        }
        let mut pan = Vec2::ZERO;
        for event in &input.events {
            let Event::MouseWheel {
                unit,
                delta,
                phase,
                modifiers,
            } = event
            else {
                continue;
            };
            gesture.last_event = input.time;
            match phase {
                // Only touchpads report phases. Like egui, the delta of a
                // start or end event is not applied.
                TouchPhase::Start => {
                    gesture.source = Some(ScrollSource::Touchpad);
                    continue;
                }
                TouchPhase::End | TouchPhase::Cancel => {
                    gesture.source = None;
                    continue;
                }
                TouchPhase::Move => {}
            }
            let source = *gesture.source.get_or_insert(classify(*unit, *delta));
            if source == ScrollSource::Touchpad && !modifiers.matches_any(zoom_modifier) {
                pan += match unit {
                    MouseWheelUnit::Point => *delta,
                    MouseWheelUnit::Line => *delta * line_points,
                    MouseWheelUnit::Page => *delta * input.viewport_rect().height(),
                };
            }
        }
        pan
    });
    ctx.data_mut(|data| data.insert_temp(id, gesture));
    FrameScroll {
        source: gesture.source,
        touchpad_pan,
    }
}

/// The device behind the first event of a gesture without phases.
fn classify(unit: MouseWheelUnit, delta: Vec2) -> ScrollSource {
    match unit {
        MouseWheelUnit::Point => ScrollSource::Touchpad,
        // A wheel notch is a whole line on one axis; a touchpad moves in
        // fractions of a line and on both axes.
        MouseWheelUnit::Line if delta.x != 0.0 || delta.y.fract() != 0.0 => ScrollSource::Touchpad,
        MouseWheelUnit::Line | MouseWheelUnit::Page => ScrollSource::Wheel,
    }
}
