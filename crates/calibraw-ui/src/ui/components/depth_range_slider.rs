use super::feathered_range::{self, round_to, FeatheredRange, RangeEdit, RangeHandle, RangePoints};
use crate::pipeline::DepthRangeSettings;
use eframe::egui::{Response, Ui};

/// Stored precision of dragged values.
const DECIMALS: i32 = 3;

impl FeatheredRange for DepthRangeSettings {
    /// `near` and `far` are ramp centres and the feathers ramp widths. A
    /// fade may run off the track; it is drawn at the track end. (An end at
    /// exactly 0 or 1 has no ramp at all; its stored feather is kept for when
    /// the end moves inwards again.)
    fn points(&self) -> RangePoints {
        let near_half = self.near_feather.clamp(0.0, 1.0) * 0.5;
        let far_half = self.far_feather.clamp(0.0, 1.0) * 0.5;
        RangePoints {
            fade_in: self.near - near_half,
            full_from: self.near + near_half,
            full_to: self.far - far_half,
            fade_out: self.far + far_half,
        }
    }

    fn set_points(&mut self, edit: RangeEdit) {
        let round = |value| round_to(value, DECIMALS);
        if let Some(edit) = edit.start {
            let half = self.near_feather.clamp(0.0, 1.0) * 0.5;
            match edit.resolve(self.near + half, round) {
                (full_from, None) => self.near = full_from - half,
                (full_from, Some(fade_in)) => {
                    self.near_feather = full_from - fade_in;
                    self.near = (fade_in + full_from) * 0.5;
                }
            }
        }
        if let Some(edit) = edit.end {
            let half = self.far_feather.clamp(0.0, 1.0) * 0.5;
            match edit.resolve(self.far - half, round) {
                (full_to, None) => self.far = full_to + half,
                (full_to, Some(fade_out)) => {
                    self.far_feather = fade_out - full_to;
                    self.far = (full_to + fade_out) * 0.5;
                }
            }
        }
        self.near = self.near.max(0.0);
        self.far = self.far.min(1.0);
        // Rounding a moved point must not leave Near a hair past Far. The end
        // this step changed yields (Far, after a push) so an untouched end stays exact.
        if self.near > self.far {
            if edit.end.is_some() {
                self.far = self.near;
            } else {
                self.near = self.far;
            }
        }
    }

    fn handle_text(&self, handle: RangeHandle) -> String {
        match handle {
            RangeHandle::Start => format!("Near {:.2}", self.near),
            RangeHandle::End => format!("Far {:.2}", self.far),
            RangeHandle::StartFeather => format!("Near feather {:.2}", self.near_feather),
            RangeHandle::EndFeather => format!("Far feather {:.2}", self.far_feather),
        }
    }

    /// Near and Far stay on the track: a fade already off it (older
    /// settings) that a solid handle squeezes narrows further, so Near and
    /// Far still reach the track ends. Near must not pass Far (the mask
    /// clamps Far to Near), while the two fades may overlap. A dragged handle
    /// that would carry one centre past the other stops where they meet.
    fn constrain(&self, points: RangePoints, moved: RangeHandle) -> RangePoints {
        let mut limited = points;
        limited.fade_in = points.fade_in.max(-points.full_from);
        limited.fade_out = points.fade_out.min(2.0 - points.full_to);
        let near = (limited.fade_in + limited.full_from) * 0.5;
        let far = (limited.full_to + limited.fade_out) * 0.5;
        if near <= far {
            return limited;
        }
        match moved {
            // A fade handle moves alone.
            RangeHandle::StartFeather => {
                limited.fade_in = (2.0 * far - limited.full_from).clamp(0.0, limited.full_from);
            }
            RangeHandle::EndFeather => {
                limited.fade_out = (2.0 * near - limited.full_to).clamp(limited.full_to, 1.0);
            }
            // A solid handle moves its end, fade included, back to the meeting point.
            RangeHandle::Start => {
                limited.fade_in -= near - far;
                limited.full_from -= near - far;
            }
            RangeHandle::End => {
                limited.full_to += near - far;
                limited.fade_out += near - far;
            }
        }
        limited
    }

    fn weight(&self, t: f32) -> f32 {
        DepthRangeSettings::weight(self, t)
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

fn range_track(ui: &mut Ui, range: &mut DepthRangeSettings) -> Response {
    let background = feathered_range::plain_track_background(ui.visuals());
    feathered_range::feathered_range_track(
        ui,
        range,
        "Depth range",
        "Near is on the left. Solid handles set where the selection is at full strength; hollow handles set where each end's fade begins.",
        background,
    )
}

pub(crate) fn depth_range_slider(ui: &mut Ui, range: &mut DepthRangeSettings) -> bool {
    let before = *range;
    ui.label("Depth range");
    range_track(ui, range);
    *range != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::components::feathered_range::{
        drag_range, handle_position, nearest_handle, track_rect, HandleDrag,
    };
    use eframe::egui::{self, pos2, vec2, Pos2, Rect};

    fn show(
        ctx: &egui::Context,
        range: &mut DepthRangeSettings,
        events: Vec<egui::Event>,
        enabled: bool,
    ) -> (Response, Rect) {
        let mut result = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(360.0, 240.0))),
                events,
                ..Default::default()
            },
            |ui| {
                ui.set_width(320.0);
                ui.add_enabled_ui(enabled, |ui| {
                    let response = range_track(ui, range);
                    let track = track_rect(ui, response.rect);
                    result = Some((response, track));
                });
            },
        );
        result.unwrap()
    }

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn close(actual: DepthRangeSettings, expected: DepthRangeSettings, context: &str) {
        for (a, b) in [
            (actual.near, expected.near),
            (actual.far, expected.far),
            (actual.near_feather, expected.near_feather),
            (actual.far_feather, expected.far_feather),
        ] {
            assert!(
                (a - b).abs() < 2e-3,
                "{context}: {actual:?} != {expected:?}"
            );
        }
    }

    #[test]
    fn solid_handles_sit_on_the_top_line_and_hollow_ones_on_the_bottom() {
        let range = DepthRangeSettings {
            near: 0.25,
            far: 0.75,
            near_feather: 0.1,
            far_feather: 0.3,
        };
        let track = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 50.0));
        for (handle, x, y) in [
            (RangeHandle::Start, 0.30, 0.0),
            (RangeHandle::StartFeather, 0.20, 50.0),
            (RangeHandle::End, 0.60, 0.0),
            (RangeHandle::EndFeather, 0.90, 50.0),
        ] {
            let position = handle_position(track, &range, handle);
            assert!((position.x - x * 100.0).abs() < 1e-3, "{handle:?}");
            assert_eq!(position.y, y, "{handle:?}");
            // The curve passes through each handle.
            let weight = if y == 0.0 { 1.0 } else { 0.0 };
            assert!((range.weight(x) - weight).abs() < 1e-4, "{handle:?}");
        }
    }

    #[test]
    fn dragging_each_handle_changes_only_its_own_end() {
        let initial = DepthRangeSettings {
            near: 0.25,
            far: 0.75,
            near_feather: 0.1,
            far_feather: 0.3,
        };
        for (handle, expected) in [
            // The solid handles move their end with its feather.
            (
                RangeHandle::Start,
                DepthRangeSettings {
                    near: 0.3,
                    ..initial
                },
            ),
            (
                RangeHandle::End,
                DepthRangeSettings {
                    far: 0.8,
                    ..initial
                },
            ),
            // The full-strength point (0.30) stays; the fade starts at 0.25.
            (
                RangeHandle::StartFeather,
                DepthRangeSettings {
                    near: 0.275,
                    near_feather: 0.05,
                    ..initial
                },
            ),
            // The full-strength point (0.60) stays; the fade ends at 0.95.
            (
                RangeHandle::EndFeather,
                DepthRangeSettings {
                    far: 0.775,
                    far_feather: 0.35,
                    ..initial
                },
            ),
        ] {
            let ctx = egui::Context::default();
            let mut range = initial;
            let (_, track) = show(&ctx, &mut range, vec![], true);
            let start = handle_position(track, &range, handle);
            show(
                &ctx,
                &mut range,
                vec![egui::Event::PointerMoved(start), button(start, true)],
                true,
            );
            let end = start + vec2(track.width() * 0.05, 0.0);
            let (response, _) = show(&ctx, &mut range, vec![egui::Event::PointerMoved(end)], true);
            assert!(response.changed(), "{handle:?}");
            close(range, expected, &format!("{handle:?}"));
            show(&ctx, &mut range, vec![button(end, false)], true);
        }
    }

    #[test]
    fn crossing_handles_push_the_other_end_with_its_feather() {
        let initial = DepthRangeSettings {
            near: 0.2,
            far: 0.8,
            ..Default::default()
        };
        let mut range = initial;
        let mut drag = HandleDrag::new(&range, RangeHandle::Start);
        // The near end pushes the far end with both feathers unchanged while
        // the far fade fits on the track.
        drag.move_to(&mut range, 0.89);
        let points = range.points();
        assert!((points.full_from - 0.89).abs() < 2e-3, "{range:?}");
        assert!((points.fade_out - 0.99).abs() < 2e-3, "{range:?}");
        assert_eq!(
            (range.near_feather, range.far_feather),
            (initial.near_feather, initial.far_feather)
        );
        // Then the pushed far fade is squeezed, and both reach the end.
        drag.move_to(&mut range, 1.0);
        let points = range.points();
        assert!((points.full_from - 1.0).abs() < 1e-6, "{range:?}");
        assert_eq!(points.full_to, 1.0, "{range:?}");
        assert_eq!(range.near_feather, initial.near_feather);
        assert_eq!((range.far, range.far_feather), (1.0, 0.0));
    }

    #[test]
    fn solid_handles_reach_both_track_ends_with_a_feather_set() {
        let initial = DepthRangeSettings {
            near: 0.3,
            far: 0.7,
            near_feather: 0.2,
            far_feather: 0.2,
        };
        let mut range = initial;
        drag_range(&mut range, &initial, RangeHandle::Start, -1.0);
        assert_eq!(range.points().full_from, 0.0, "{range:?}");
        assert_eq!((range.near, range.near_feather), (0.0, 0.0));
        let mut range = initial;
        drag_range(&mut range, &initial, RangeHandle::End, 1.0);
        assert_eq!(range.points().full_to, 1.0, "{range:?}");
        assert_eq!((range.far, range.far_feather), (1.0, 0.0));
        // An older near fade already off the track (at -0.2) narrows too.
        let initial = DepthRangeSettings {
            near: 0.1,
            near_feather: 0.6,
            ..initial
        };
        let mut range = initial;
        let mut drag = HandleDrag::new(&range, RangeHandle::Start);
        for target in [0.3, 0.1, 0.0] {
            drag.move_to(&mut range, target);
            assert!(
                (range.points().full_from - target).abs() < 2e-3,
                "{target}: {range:?}"
            );
            assert!(range.near >= 0.0, "{target}: {range:?}");
        }
        assert_eq!((range.near, range.near_feather), (0.0, 0.0));
    }

    #[test]
    fn feathers_change_only_when_their_own_handle_is_dragged() {
        for start in [
            DepthRangeSettings::default(),
            DepthRangeSettings {
                near: 0.3,
                far: 0.7,
                near_feather: 0.4,
                far_feather: 0.4,
            },
            DepthRangeSettings {
                near: 0.0,
                far: 1.0,
                near_feather: 0.5,
                far_feather: 0.5,
            },
            DepthRangeSettings {
                near: 0.5,
                far: 0.5,
                near_feather: 0.0,
                far_feather: 0.0,
            },
        ] {
            for handle in RangeHandle::ALL {
                for delta in [-2.0, -0.2, -0.01, 0.01, 0.2, 2.0] {
                    let mut range = start;
                    drag_range(&mut range, &start, handle, delta);
                    let context = format!("{handle:?} {delta} from {start:?}: {range:?}");
                    assert!(0.0 <= range.near && range.near <= range.far, "{context}");
                    assert!(range.far <= 1.0, "{context}");
                    match handle {
                        // A solid handle squeezes a feather only where that
                        // end's carried fade would leave the track.
                        RangeHandle::Start | RangeHandle::End => {
                            let points = range.points();
                            let squeezed = [
                                (
                                    range.near_feather,
                                    start.near_feather,
                                    points.fade_in <= 1e-3,
                                ),
                                (
                                    range.far_feather,
                                    start.far_feather,
                                    points.fade_out >= 1.0 - 1e-3,
                                ),
                            ];
                            for (feather, before, at_track_end) in squeezed {
                                assert!(
                                    feather == before || (at_track_end && feather < before),
                                    "{context}"
                                );
                            }
                        }
                        RangeHandle::StartFeather => {
                            let (before, after) = (start.points(), range.points());
                            assert!(
                                (after.full_from - before.full_from).abs() < 1e-5,
                                "{context}"
                            );
                            assert_eq!(
                                (range.far, range.far_feather),
                                (start.far, start.far_feather)
                            );
                        }
                        RangeHandle::EndFeather => {
                            let (before, after) = (start.points(), range.points());
                            assert!((after.full_to - before.full_to).abs() < 1e-5, "{context}");
                            assert_eq!(
                                (range.near, range.near_feather),
                                (start.near, start.near_feather)
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn both_handles_in_a_corner_can_be_grabbed() {
        let track = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 54.0));
        let range = DepthRangeSettings {
            near: 0.0,
            far: 1.0,
            near_feather: 0.2,
            far_feather: 0.2,
        };
        for (pointer, expected) in [
            (pos2(2.0, track.top()), RangeHandle::Start),
            (pos2(2.0, track.bottom()), RangeHandle::StartFeather),
            (pos2(298.0, track.top()), RangeHandle::End),
            (pos2(298.0, track.bottom()), RangeHandle::EndFeather),
        ] {
            assert_eq!(nearest_handle(track, &range, pointer), expected);
        }
        // Dragging the solid handle inwards brings its fade along unchanged.
        let mut moved = range;
        drag_range(&mut moved, &range, RangeHandle::Start, 0.2);
        assert!((moved.near - 0.2).abs() < 2e-3, "{moved:?}");
        assert_eq!(moved.near_feather, range.near_feather);
    }

    #[test]
    fn fade_handle_follows_the_pointer_from_the_track_end() {
        // The near fade starts off the track (at -0.2), so its handle sits at
        // 0. Dragging it right moves it at once, with no dead zone while the
        // hidden excess is used up.
        let initial = DepthRangeSettings {
            near: 0.1,
            far: 0.9,
            near_feather: 0.6,
            far_feather: 0.1,
        };
        assert_eq!(
            HandleDrag::new(&initial, RangeHandle::StartFeather).place(),
            0.0
        );
        let mut range = initial;
        drag_range(&mut range, &initial, RangeHandle::StartFeather, 0.05);
        assert!((range.points().fade_in - 0.05).abs() < 2e-3);
        assert!((range.points().full_from - 0.4).abs() < 2e-3);
        // The far end was not touched and keeps its exact values.
        assert_eq!(
            (range.far, range.far_feather),
            (initial.far, initial.far_feather)
        );
    }

    #[test]
    fn the_selected_handle_is_an_accessible_slider_that_follows_its_actions() {
        use egui::accesskit::{Action, ActionData, ActionRequest, Node, NodeId, Role};

        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut range = DepthRangeSettings::default();
        let mut run = |events| -> (NodeId, Node, DepthRangeSettings) {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(360.0, 240.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(320.0);
                    range_track(ui, &mut range);
                },
            );
            let update = output
                .platform_output
                .accesskit_update
                .expect("AccessKit output is enabled");
            let (id, node) = update
                .nodes
                .into_iter()
                .find(|(_, node)| node.role() == Role::Slider)
                .expect("the range is a slider");
            (id, node, range)
        };
        // The near solid handle is selected first.
        let (id, slider, before) = run(Vec::new());
        assert_eq!(slider.label(), Some("Depth range"));
        assert_eq!(slider.value(), Some("Near 0.00"));
        assert_eq!(
            slider.description(),
            Some("Near 0.00, Far 0.50, Near feather 0.10, Far feather 0.10")
        );
        for action in [
            Action::Focus,
            Action::Increment,
            Action::Decrement,
            Action::SetValue,
        ] {
            assert!(slider.supports_action(action), "{action:?}");
        }
        let request = |action, data| {
            egui::Event::AccessKitActionRequest(ActionRequest {
                action,
                target_tree: egui::accesskit::TreeId::ROOT,
                target_node: id,
                data,
            })
        };
        // Increment moves the selected handle one key step, keeping its feather.
        let (_, _, moved) = run(vec![request(Action::Increment, None)]);
        assert!(
            (moved.points().full_from - before.points().full_from - 0.01).abs() < 2e-3,
            "{moved:?}"
        );
        assert_eq!(moved.near_feather, before.near_feather);
        // SetValue places it on the track.
        run(vec![request(
            Action::SetValue,
            Some(ActionData::NumericValue(0.3)),
        )]);
        let (_, slider, moved) = run(Vec::new());
        assert!((moved.points().full_from - 0.3).abs() < 2e-3, "{moved:?}");
        assert_eq!(slider.value(), Some("Near 0.25"));
        assert_eq!(moved.far, before.far);
    }

    #[test]
    fn vertical_scroll_gesture_does_not_edit_the_range() {
        let ctx = egui::Context::default();
        let mut range = DepthRangeSettings::default();
        let initial = range;
        let (_, track) = show(&ctx, &mut range, vec![], true);
        let start = handle_position(track, &range, RangeHandle::End);
        show(
            &ctx,
            &mut range,
            vec![egui::Event::PointerMoved(start), button(start, true)],
            true,
        );
        show(
            &ctx,
            &mut range,
            vec![egui::Event::PointerMoved(start + vec2(5.0, -25.0))],
            true,
        );
        assert_eq!(range, initial);
        assert!(!moduwu_design::slider_scroll_locked(&ctx));
    }

    #[test]
    fn disabled_range_cannot_be_dragged() {
        let ctx = egui::Context::default();
        let mut range = DepthRangeSettings::default();
        let initial = range;
        let (_, track) = show(&ctx, &mut range, vec![], false);
        let start = handle_position(track, &range, RangeHandle::End);
        show(
            &ctx,
            &mut range,
            vec![egui::Event::PointerMoved(start), button(start, true)],
            false,
        );
        show(
            &ctx,
            &mut range,
            vec![egui::Event::PointerMoved(start + vec2(30.0, 0.0))],
            false,
        );
        assert_eq!(range, initial);
    }

    #[test]
    fn arrow_keys_move_the_selected_handle_and_up_down_choose_another() {
        let ctx = egui::Context::default();
        let mut range = DepthRangeSettings::default();
        let initial = range;
        let (_, track) = show(&ctx, &mut range, vec![], true);
        let far = handle_position(track, &range, RangeHandle::End);
        for pressed in [true, false] {
            show(
                &ctx,
                &mut range,
                vec![egui::Event::PointerMoved(far), button(far, pressed)],
                true,
            );
        }
        // egui applies the arrow-key focus lock from the frame after focus.
        show(&ctx, &mut range, vec![], true);
        let key = |key, pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        show(
            &ctx,
            &mut range,
            vec![key(egui::Key::ArrowRight, true)],
            true,
        );
        assert!((range.far - (initial.far + 0.01)).abs() < 1e-4, "{range:?}");
        assert_eq!(range.far_feather, initial.far_feather);
        assert_eq!(range.near, initial.near);
        show(
            &ctx,
            &mut range,
            vec![
                key(egui::Key::ArrowRight, false),
                key(egui::Key::ArrowUp, true),
            ],
            true,
        );
        show(
            &ctx,
            &mut range,
            vec![
                key(egui::Key::ArrowUp, false),
                key(egui::Key::ArrowRight, true),
            ],
            true,
        );
        // Up selects the near solid handle (in the corner at 0); moving it
        // right opens the near end of the range.
        assert!(range.near > 0.0, "{range:?}");
    }

    #[test]
    fn pointer_drag_that_pushes_a_handle_leaves_it_there_when_reversed() {
        let ctx = egui::Context::default();
        let mut range = DepthRangeSettings {
            near: 0.2,
            far: 0.5,
            near_feather: 0.0,
            far_feather: 0.0,
        };
        let (_, track) = show(&ctx, &mut range, vec![], true);
        let start = handle_position(track, &range, RangeHandle::Start);
        show(
            &ctx,
            &mut range,
            vec![egui::Event::PointerMoved(start), button(start, true)],
            true,
        );
        // Past the far end at 0.8: it is pushed along.
        for t in [0.3, 0.6, 0.8] {
            let pointer = pos2(egui::lerp(track.x_range(), t), start.y);
            show(
                &ctx,
                &mut range,
                vec![egui::Event::PointerMoved(pointer)],
                true,
            );
        }
        assert!((range.far - 0.8).abs() < 2e-3, "{range:?}");
        // Back to 0.3: the far end stays at 0.8.
        for t in [0.6, 0.3] {
            let pointer = pos2(egui::lerp(track.x_range(), t), start.y);
            show(
                &ctx,
                &mut range,
                vec![egui::Event::PointerMoved(pointer)],
                true,
            );
        }
        assert!((range.points().full_from - 0.3).abs() < 2e-3, "{range:?}");
        assert!((range.far - 0.8).abs() < 2e-3, "{range:?}");
    }

    #[test]
    fn overlapping_fades_are_edited_as_they_are() {
        // The two fades overlap, so full strength is never reached.
        let initial = DepthRangeSettings {
            near: 0.45,
            far: 0.55,
            near_feather: 0.4,
            far_feather: 0.4,
        };
        // Moving the near fade changes only the near end, and its full-strength
        // point stays put.
        let mut range = initial;
        drag_range(&mut range, &initial, RangeHandle::StartFeather, 0.05);
        assert_eq!((range.far, range.far_feather), (0.55, 0.4));
        assert!((range.points().fade_in - 0.30).abs() < 1e-4, "{range:?}");
        assert!((range.points().full_from - 0.65).abs() < 1e-4, "{range:?}");
        // Moving the near solid handle does not push the far end either.
        let mut range = initial;
        drag_range(&mut range, &initial, RangeHandle::Start, -0.1);
        assert_eq!((range.far, range.far_feather), (0.55, 0.4));
        // Near cannot pass Far: the near fade stops where the centres meet,
        // and the fades may still overlap.
        let mut range = initial;
        drag_range(&mut range, &initial, RangeHandle::StartFeather, 0.35);
        assert!(range.near <= range.far, "{range:?}");
        assert_eq!((range.far, range.far_feather), (0.55, 0.4));
        let mut range = initial;
        drag_range(&mut range, &initial, RangeHandle::EndFeather, -0.35);
        assert!(range.near <= range.far, "{range:?}");
        assert_eq!((range.near, range.near_feather), (0.45, 0.4));
    }

    #[test]
    fn every_handle_stays_grabbable_when_fades_overlap_completely() {
        // Each solid handle is right above the opposite end's fade handle.
        let range = DepthRangeSettings {
            near: 0.5,
            far: 0.5,
            near_feather: 0.5,
            far_feather: 0.5,
        };
        let track = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 60.0));
        for handle in RangeHandle::ALL {
            let at = handle_position(track, &range, handle);
            assert_eq!(
                nearest_handle(track, &range, at),
                handle,
                "{handle:?} at {at:?}"
            );
        }
    }
}
