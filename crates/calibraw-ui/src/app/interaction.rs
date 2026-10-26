//! Whether the user is in the middle of a gesture, which background work waits
//! for or settles after: the editor saves and records history between
//! gestures, and renders the idle full-resolution preview after them.

use super::egui;

/// A pointer button or a touch is down: a drag, a press or a resting finger.
pub(crate) fn pointer_engaged(ctx: &egui::Context) -> bool {
    ctx.input(|input| input.pointer.any_down() || input.any_touches())
}

/// A widget is being dragged: a slider, a curve point, a wheel. A press that
/// has not moved yet is a click, not a drag.
pub(crate) fn control_dragged(ctx: &egui::Context) -> bool {
    ctx.dragged_id().is_some()
}
