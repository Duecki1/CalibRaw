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

#[cfg(test)]
mod tests {
    use super::*;

    fn engaged_after(events: Vec<egui::Event>) -> bool {
        let ctx = egui::Context::default();
        let mut engaged = false;
        let _ = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| engaged = pointer_engaged(ui.ctx()),
        );
        engaged
    }

    #[test]
    fn a_pressed_button_engages_the_pointer_until_it_is_released() {
        let at = egui::pos2(10.0, 10.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        assert!(!engaged_after(vec![]));
        assert!(engaged_after(vec![
            egui::Event::PointerMoved(at),
            button(true)
        ]));
        assert!(!engaged_after(vec![
            egui::Event::PointerMoved(at),
            button(true),
            button(false)
        ]));
    }
}
