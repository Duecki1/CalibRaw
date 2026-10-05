use super::feathered_range::{self, FeatheredRange, RangeHandle};
use crate::pipeline::DepthRangeSettings;
use eframe::egui::{Response, Ui};
use moduwu_design::NumberField;

impl FeatheredRange for DepthRangeSettings {
    /// A feather handle marks where its ramp reaches full strength and never
    /// leaves the track, even when the numeric feather is wider than the room
    /// left.
    fn handle_value(&self, handle: RangeHandle) -> f32 {
        let value = match handle {
            RangeHandle::Start => self.near,
            RangeHandle::End => self.far,
            RangeHandle::StartFeather => self.near + self.near_feather * 0.5,
            RangeHandle::EndFeather => self.far - self.far_feather * 0.5,
        };
        value.clamp(0.0, 1.0)
    }

    /// A feather does nothing at the track end its endpoint sits on (there is
    /// no depth beyond it to fade into), so its handle is hidden there.
    fn handle_active(&self, handle: RangeHandle) -> bool {
        match handle {
            RangeHandle::Start | RangeHandle::End => true,
            RangeHandle::StartFeather => self.near > 0.0,
            RangeHandle::EndFeather => self.far < 1.0,
        }
    }

    fn drag(&mut self, start: &Self, handle: RangeHandle, delta: f32) {
        let target = start.handle_value(handle) + delta;
        match handle {
            RangeHandle::Start => self.near = target.clamp(0.0, start.far),
            RangeHandle::End => self.far = target.clamp(start.near, 1.0),
            RangeHandle::StartFeather => {
                self.near_feather = (2.0 * (target - start.near)).clamp(0.0, 1.0);
            }
            RangeHandle::EndFeather => {
                self.far_feather = (2.0 * (start.far - target)).clamp(0.0, 1.0);
            }
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
        "Drag the lower circles to select near and far depth. Drag each upper diamond to change that end’s feather independently; an end at 0 or 1 has no feather. Double-click to reset.",
        background,
    )
}

pub(crate) fn depth_range_slider(ui: &mut Ui, range: &mut DepthRangeSettings) -> bool {
    let before = *range;
    ui.horizontal(|ui| {
        ui.strong("Depth range");
        if ui.small_button("Reset").clicked() {
            *range = DepthRangeSettings::default();
        }
    });
    range_track(ui, range);
    // Numeric entry also keeps every handle reachable when endpoints overlap,
    // and exposes the full feather width even when its ramp extends off-track.
    ui.columns(2, |columns| {
        columns[0].horizontal(|ui| {
            ui.label("Near");
            ui.add(
                NumberField::new(&mut range.near, 0.0..=range.far)
                    .speed(0.005)
                    .decimals(2),
            );
        });
        columns[1].horizontal(|ui| {
            ui.label("Far");
            ui.add(
                NumberField::new(&mut range.far, range.near..=1.0)
                    .speed(0.005)
                    .decimals(2),
            );
        });
        let near_feather_active = range.handle_active(RangeHandle::StartFeather);
        let far_feather_active = range.handle_active(RangeHandle::EndFeather);
        columns[0].label("Near feather");
        columns[0]
            .add_enabled(
                near_feather_active,
                NumberField::new(&mut range.near_feather, 0.0..=1.0)
                    .speed(0.005)
                    .decimals(2),
            )
            .on_disabled_hover_text("Raise Near above 0 to feather the near end.");
        columns[1].label("Far feather");
        columns[1]
            .add_enabled(
                far_feather_active,
                NumberField::new(&mut range.far_feather, 0.0..=1.0)
                    .speed(0.005)
                    .decimals(2),
            )
            .on_disabled_hover_text("Lower Far below 1 to feather the far end.");
    });
    *range != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::components::feathered_range::{handle_position, nearest_handle, TRACK_INSET};
    use eframe::egui::{self, pos2, vec2, Pos2, Rect};

    fn show(
        ctx: &egui::Context,
        range: &mut DepthRangeSettings,
        events: Vec<egui::Event>,
        enabled: bool,
    ) -> Response {
        let mut response = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(360.0, 240.0))),
                events,
                ..Default::default()
            },
            |ui| {
                ui.set_width(320.0);
                ui.add_enabled_ui(enabled, |ui| response = Some(range_track(ui, range)));
            },
        );
        response.unwrap()
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
    fn dragging_each_handle_changes_only_its_own_value() {
        for handle in RangeHandle::ALL {
            let ctx = egui::Context::default();
            let mut range = DepthRangeSettings {
                near: 0.25,
                far: 0.75,
                near_feather: 0.1,
                far_feather: 0.3,
            };
            let initial = range;
            let response = show(&ctx, &mut range, vec![], true);
            let track = response.rect.shrink2(TRACK_INSET);
            let start = handle_position(track, &range, handle);
            show(
                &ctx,
                &mut range,
                vec![egui::Event::PointerMoved(start), button(start, true)],
                true,
            );
            let end = start + vec2(track.width() * 0.05, 0.0);
            let response = show(&ctx, &mut range, vec![egui::Event::PointerMoved(end)], true);
            assert!(response.changed(), "{handle:?}");
            let expected = match handle {
                RangeHandle::Start => DepthRangeSettings {
                    near: 0.3,
                    ..initial
                },
                RangeHandle::End => DepthRangeSettings {
                    far: 0.8,
                    ..initial
                },
                RangeHandle::StartFeather => DepthRangeSettings {
                    near_feather: 0.2,
                    ..initial
                },
                RangeHandle::EndFeather => DepthRangeSettings {
                    far_feather: 0.2,
                    ..initial
                },
            };
            assert!((range.near - expected.near).abs() < 1e-5, "{handle:?}");
            assert!((range.far - expected.far).abs() < 1e-5, "{handle:?}");
            assert!(
                (range.near_feather - expected.near_feather).abs() < 1e-5,
                "{handle:?}"
            );
            assert!(
                (range.far_feather - expected.far_feather).abs() < 1e-5,
                "{handle:?}"
            );
            show(&ctx, &mut range, vec![button(end, false)], true);
        }
    }

    #[test]
    fn handles_cannot_cross_and_collapsed_ranges_can_reopen() {
        let track = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 54.0));
        for value in [0.0, 0.5, 1.0] {
            let initial = DepthRangeSettings {
                near: value,
                far: value,
                ..Default::default()
            };
            let mut range = initial;
            let pointer = pos2(track.width() * value, track.bottom());
            let handle = nearest_handle(track, &range, pointer);
            range.drag(&initial, handle, if value < 1.0 { 0.2 } else { -0.2 });
            assert!(range.near < range.far);
        }
        let initial = DepthRangeSettings {
            near: 0.2,
            far: 0.8,
            ..Default::default()
        };
        let mut range = initial;
        range.drag(&initial, RangeHandle::Start, 2.0);
        assert_eq!(range.near, range.far);
        range = initial;
        range.drag(&initial, RangeHandle::End, -2.0);
        assert_eq!(range.near, range.far);
        assert_eq!(range.near_feather, initial.near_feather);
        assert_eq!(range.far_feather, initial.far_feather);
    }

    #[test]
    fn feathers_at_the_track_ends_cannot_be_grabbed() {
        let track = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 54.0));
        let range = DepthRangeSettings {
            near: 0.0,
            far: 1.0,
            near_feather: 0.2,
            far_feather: 0.2,
        };
        for pointer in [pos2(30.0, track.top()), pos2(270.0, track.top())] {
            let handle = nearest_handle(track, &range, pointer);
            assert!(
                matches!(handle, RangeHandle::Start | RangeHandle::End),
                "{handle:?}"
            );
        }
    }

    #[test]
    fn feather_handle_follows_the_pointer_from_the_track_end() {
        // The near feather is wider than the room left, so its handle sits
        // at the right end. Dragging it left moves it at once, with no dead
        // zone while the hidden excess is used up.
        let initial = DepthRangeSettings {
            near: 0.8,
            far: 1.0,
            near_feather: 1.0,
            far_feather: 0.1,
        };
        assert_eq!(initial.handle_value(RangeHandle::StartFeather), 1.0);
        let mut range = initial;
        range.drag(&initial, RangeHandle::StartFeather, -0.05);
        assert!((range.handle_value(RangeHandle::StartFeather) - 0.95).abs() < 1e-5);
        assert!((range.near_feather - 0.3).abs() < 1e-5);
    }

    #[test]
    fn vertical_scroll_gesture_does_not_edit_the_range() {
        let ctx = egui::Context::default();
        let mut range = DepthRangeSettings::default();
        let initial = range;
        let rect = show(&ctx, &mut range, vec![], true).rect;
        let start = handle_position(rect.shrink2(TRACK_INSET), &range, RangeHandle::End);
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
        let rect = show(&ctx, &mut range, vec![], false).rect;
        let start = handle_position(rect.shrink2(TRACK_INSET), &range, RangeHandle::End);
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
}
