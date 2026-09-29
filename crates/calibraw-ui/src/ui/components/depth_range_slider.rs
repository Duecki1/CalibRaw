use crate::pipeline::DepthRangeSettings;
use eframe::egui::{self, pos2, vec2, DragValue, Pos2, Rect, Response, Sense, Stroke, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    Near,
    Far,
    NearFeather,
    FarFeather,
}

impl Handle {
    const ALL: [Self; 4] = [Self::Near, Self::Far, Self::NearFeather, Self::FarFeather];

    fn position(self, track: Rect, range: &DepthRangeSettings) -> Pos2 {
        let (value, y) = match self {
            Self::Near => (range.near, track.bottom()),
            Self::Far => (range.far, track.bottom()),
            Self::NearFeather => (range.near + range.near_feather * 0.5, track.top()),
            Self::FarFeather => (range.far - range.far_feather * 0.5, track.top()),
        };
        pos2(egui::lerp(track.x_range(), value.clamp(0.0, 1.0)), y)
    }

    fn drag(self, range: &mut DepthRangeSettings, start: DepthRangeSettings, delta: f32) {
        match self {
            Self::Near => range.near = (start.near + delta).clamp(0.0, start.far),
            Self::Far => range.far = (start.far + delta).clamp(start.near, 1.0),
            Self::NearFeather => {
                range.near_feather = (start.near_feather + 2.0 * delta).clamp(0.0, 1.0)
            }
            Self::FarFeather => {
                range.far_feather = (start.far_feather - 2.0 * delta).clamp(0.0, 1.0)
            }
        }
    }
}

#[derive(Clone, Copy)]
struct RangeDrag {
    handle: Handle,
    start_x: f32,
    initial: DepthRangeSettings,
}

fn nearest_handle(track: Rect, range: &DepthRangeSettings, pointer: Pos2) -> Handle {
    // Resolve coincident handles by the side approached, so a collapsed range
    // can be opened in either direction (including at 0 and 1).
    let prefer_far =
        pointer.x >= egui::lerp(track.x_range(), (range.near + range.far) * 0.5) && range.far < 1.0;
    Handle::ALL
        .into_iter()
        .min_by(|a, b| {
            let a_pos = a.position(track, range);
            let b_pos = b.position(track, range);
            pointer
                .distance_sq(a_pos)
                .total_cmp(&pointer.distance_sq(b_pos))
                .then_with(|| {
                    let far = |handle| matches!(handle, Handle::Far | Handle::FarFeather);
                    (far(*a) != prefer_far).cmp(&(far(*b) != prefer_far))
                })
        })
        .unwrap()
}

fn range_track(ui: &mut Ui, range: &mut DepthRangeSettings) -> Response {
    let before = *range;
    let (rect, mut response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(1.0), 82.0),
        Sense::click_and_drag(),
    );
    let track = rect.shrink2(vec2(10.0, 14.0));
    let drag_id = response.id.with("depth-range-drag");
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
            ui.data(|data| data.get_temp::<RangeDrag>(drag_id)),
            response.interact_pointer_pos(),
        ) {
            super::adjustment_slider::lock_slider_scroll(ui.ctx(), response.id);
            drag.handle.drag(
                range,
                drag.initial,
                (pointer.x - drag.start_x) / track.width().max(1.0),
            );
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|data| data.remove::<RangeDrag>(drag_id));
    }
    if response.double_clicked() {
        *range = DepthRangeSettings::default();
    }
    if *range != before {
        response.mark_changed();
    }

    let painter = ui.painter_at(rect);
    let accent = ui.visuals().selection.bg_fill;
    painter.rect_filled(track, 3.0, ui.visuals().extreme_bg_color);
    for step in 1..4 {
        let x = egui::lerp(track.x_range(), step as f32 / 4.0);
        painter.line_segment(
            [pos2(x, track.top()), pos2(x, track.bottom())],
            Stroke::new(1.0, ui.visuals().faint_bg_color),
        );
    }
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
    for handle in Handle::ALL {
        let center = handle.position(track, range);
        let stroke = Stroke::new(1.5, ui.visuals().text_color());
        match handle {
            Handle::Near | Handle::Far => {
                painter.line_segment(
                    [pos2(center.x, track.top()), center],
                    Stroke::new(1.0, accent),
                );
                painter.circle(center, 6.0, accent, stroke);
            }
            Handle::NearFeather | Handle::FarFeather => {
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
            }
        }
    }
    response.on_hover_text("Drag the lower circles to select near and far depth. Drag each upper diamond to change that end’s feather independently. Double-click to reset.")
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
                DragValue::new(&mut range.near)
                    .range(0.0..=range.far)
                    .speed(0.005)
                    .fixed_decimals(2),
            );
        });
        columns[1].horizontal(|ui| {
            ui.label("Far");
            ui.add(
                DragValue::new(&mut range.far)
                    .range(range.near..=1.0)
                    .speed(0.005)
                    .fixed_decimals(2),
            );
        });
        columns[0].label("Near feather");
        columns[0].add(
            DragValue::new(&mut range.near_feather)
                .range(0.0..=1.0)
                .speed(0.005)
                .fixed_decimals(2),
        );
        columns[1].label("Far feather");
        columns[1].add(
            DragValue::new(&mut range.far_feather)
                .range(0.0..=1.0)
                .speed(0.005)
                .fixed_decimals(2),
        );
    });
    *range != before
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for handle in Handle::ALL {
            let ctx = egui::Context::default();
            let mut range = DepthRangeSettings {
                near: 0.25,
                far: 0.75,
                near_feather: 0.1,
                far_feather: 0.3,
            };
            let initial = range;
            let response = show(&ctx, &mut range, vec![], true);
            let track = response.rect.shrink2(vec2(10.0, 14.0));
            let start = handle.position(track, &range);
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
                Handle::Near => DepthRangeSettings {
                    near: 0.3,
                    ..initial
                },
                Handle::Far => DepthRangeSettings {
                    far: 0.8,
                    ..initial
                },
                Handle::NearFeather => DepthRangeSettings {
                    near_feather: 0.2,
                    ..initial
                },
                Handle::FarFeather => DepthRangeSettings {
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
            handle.drag(&mut range, initial, if value < 1.0 { 0.2 } else { -0.2 });
            assert!(range.near < range.far);
        }
        let initial = DepthRangeSettings {
            near: 0.2,
            far: 0.8,
            ..Default::default()
        };
        let mut range = initial;
        Handle::Near.drag(&mut range, initial, 2.0);
        assert_eq!(range.near, range.far);
        range = initial;
        Handle::Far.drag(&mut range, initial, -2.0);
        assert_eq!(range.near, range.far);
        assert_eq!(range.near_feather, initial.near_feather);
        assert_eq!(range.far_feather, initial.far_feather);
    }

    #[test]
    fn vertical_scroll_gesture_does_not_edit_the_range() {
        let ctx = egui::Context::default();
        let mut range = DepthRangeSettings::default();
        let initial = range;
        let rect = show(&ctx, &mut range, vec![], true).rect;
        let start = Handle::Far.position(rect.shrink2(vec2(10.0, 14.0)), &range);
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
        assert!(!super::super::adjustment_slider::slider_scroll_locked(&ctx));
    }

    #[test]
    fn disabled_range_cannot_be_dragged() {
        let ctx = egui::Context::default();
        let mut range = DepthRangeSettings::default();
        let initial = range;
        let rect = show(&ctx, &mut range, vec![], false).rect;
        let start = Handle::Far.position(rect.shrink2(vec2(10.0, 14.0)), &range);
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
