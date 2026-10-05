use super::feathered_range::{self, round_to, FeatheredRange, RangeField, RangePoints};
use crate::pipeline::DepthRangeSettings;
use eframe::egui::{Response, Ui};

/// Stored precision of dragged values; the fields show two decimals.
const DECIMALS: i32 = 3;

impl FeatheredRange for DepthRangeSettings {
    /// `near` and `far` are ramp centres and the feathers ramp widths. An end
    /// at 0 or 1 has no ramp (the weight is a step), so its fade sits on it.
    fn points(&self) -> RangePoints {
        let near_half = if self.near > 0.0 {
            self.near_feather.clamp(0.0, 1.0) * 0.5
        } else {
            0.0
        };
        let far_half = if self.far < 1.0 {
            self.far_feather.clamp(0.0, 1.0) * 0.5
        } else {
            0.0
        };
        RangePoints {
            fade_in: (self.near - near_half).clamp(0.0, 1.0),
            full_from: (self.near + near_half).clamp(0.0, 1.0),
            full_to: (self.far - far_half).clamp(0.0, 1.0),
            fade_out: (self.far + far_half).clamp(0.0, 1.0),
        }
    }

    /// Any ordered points on the track are a valid depth range. Only the end
    /// that moved is rewritten, so the other keeps its exact values.
    fn set_points(&mut self, points: RangePoints) {
        // Round the handle places; centres and widths follow exactly from
        // them, so a handle dragged into a corner stays exactly there.
        // Only points that moved are rounded, so a handle that was not touched
        // keeps its exact place.
        let before = self.points();
        let place = |now: f32, was: f32| {
            if now == was {
                was
            } else {
                round_to(now, DECIMALS)
            }
        };
        if points.fade_in != before.fade_in || points.full_from != before.full_from {
            let fade_in = place(points.fade_in, before.fade_in);
            let full_from = place(points.full_from, before.full_from);
            self.near = (fade_in + full_from) * 0.5;
            self.near_feather = full_from - fade_in;
        }
        if points.full_to != before.full_to || points.fade_out != before.fade_out {
            let full_to = place(points.full_to, before.full_to);
            let fade_out = place(points.fade_out, before.fade_out);
            self.far = (full_to + fade_out) * 0.5;
            self.far_feather = fade_out - full_to;
        }
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
    let value_text = format!(
        "near {:.2}, far {:.2}, near feather {:.2}, far feather {:.2}",
        range.near, range.far, range.near_feather, range.far_feather
    );
    feathered_range::feathered_range_track(
        ui,
        range,
        "Depth range",
        value_text,
        "Near is on the left. Solid handles set where the selection is at full strength; hollow handles set where each end's fade begins.",
        background,
    )
}

pub(crate) fn depth_range_slider(ui: &mut Ui, range: &mut DepthRangeSettings) -> bool {
    let before = *range;
    ui.label("Depth range");
    range_track(ui, range);
    let (near, far) = (range.near, range.far);
    feathered_range::range_fields(
        ui,
        vec![
            RangeField {
                label: "Near",
                value: &mut range.near,
                range: 0.0..=far,
                decimals: 2,
                speed: 0.005,
                disabled_reason: None,
            },
            RangeField {
                label: "Far",
                value: &mut range.far,
                range: near..=1.0,
                decimals: 2,
                speed: 0.005,
                disabled_reason: None,
            },
            RangeField {
                label: "Near feather",
                value: &mut range.near_feather,
                range: 0.0..=1.0,
                decimals: 2,
                speed: 0.005,
                disabled_reason: (near <= 0.0)
                    .then_some("Raise Near above 0 to feather the near end."),
            },
            RangeField {
                label: "Far feather",
                value: &mut range.far_feather,
                range: 0.0..=1.0,
                decimals: 2,
                speed: 0.005,
                disabled_reason: (far >= 1.0)
                    .then_some("Lower Far below 1 to feather the far end."),
            },
        ],
    );
    *range != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::components::feathered_range::RangeHandle;
    use crate::ui::components::feathered_range::{
        drag_range, handle_position, nearest_handle, track_rect,
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
            // Full strength from 0.35; the fade still begins at 0.20.
            (
                RangeHandle::Start,
                DepthRangeSettings {
                    near: 0.275,
                    near_feather: 0.15,
                    ..initial
                },
            ),
            // Full strength to 0.65; the fade still ends at 0.90.
            (
                RangeHandle::End,
                DepthRangeSettings {
                    far: 0.775,
                    far_feather: 0.25,
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
    fn crossing_handles_push_the_others_along() {
        let initial = DepthRangeSettings {
            near: 0.2,
            far: 0.8,
            ..Default::default()
        };
        let mut range = initial;
        drag_range(&mut range, &initial, RangeHandle::Start, 2.0);
        let points = range.points();
        assert_eq!(points.full_from, 1.0);
        assert!(points.full_to >= points.full_from && range.near <= range.far);
    }

    #[test]
    fn every_drag_keeps_the_range_valid_and_follows_the_pointer() {
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
                    assert!((0.0..=1.0).contains(&range.near_feather), "{context}");
                    assert!((0.0..=1.0).contains(&range.far_feather), "{context}");
                    // The dragged handle follows the pointer; a fade handle
                    // stops at its solid handle and moves nothing else.
                    let before = start.points();
                    let (low, high) = match handle {
                        RangeHandle::StartFeather => (0.0, before.full_from),
                        RangeHandle::EndFeather => (before.full_to, 1.0),
                        _ => (0.0, 1.0),
                    };
                    let target = (before.get(handle) + delta).clamp(low, high);
                    let after = range.points();
                    assert!((after.get(handle) - target).abs() < 2e-3, "{context}");
                    if matches!(handle, RangeHandle::StartFeather | RangeHandle::EndFeather) {
                        for other in RangeHandle::ALL.into_iter().filter(|h| *h != handle) {
                            assert!(
                                (after.get(other) - before.get(other)).abs() < 1e-6,
                                "{context}: {other:?} moved"
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
        // Dragging the solid handle out of the corner opens a fade from the
        // hollow one, which stays in the corner.
        let mut opened = range;
        drag_range(&mut opened, &range, RangeHandle::Start, 0.2);
        let points = opened.points();
        assert!((points.full_from - 0.2).abs() < 2e-3, "{opened:?}");
        assert_eq!(points.fade_in, 0.0);
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
        assert_eq!(initial.points().fade_in, 0.0);
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
        let points = range.points();
        assert!((points.full_to - (initial.points().full_to + 0.01)).abs() < 1e-4);
        assert_eq!(points.fade_out, initial.points().fade_out);
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
}
