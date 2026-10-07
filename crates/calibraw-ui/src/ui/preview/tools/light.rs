//! Relight lights on the canvas: drag a light to place it, scroll over it to
//! move it nearer or farther, double-click to reset its position. Positions
//! use the same full-image coordinates as the card's position pad.

use super::super::*;
use crate::app::{EffectComponentRef, LightHandle, LightHandleAction};
use crate::pipeline::effect_params::relight as params;
use moduwu_design::ScreenLayout;

const CORE_RADIUS: f32 = 6.0;
/// A handle's margin from the viewport edge: its full pointer reach.
const HIT_RADIUS: f32 = handles::POINT_REACH;
/// Depth ring radius for a light at the camera and at the farthest surface:
/// the nearer the light, the larger it looks.
const RING_RADIUS: [f32; 2] = [20.0, 9.0];
/// Light depth per point of vertical scrolling; Shift scrolls five times finer.
const DEPTH_PER_SCROLL_POINT: f32 = 0.1;

/// The lights the preview shows this frame and how the image is projected.
pub(in crate::ui::preview) struct LightHandleInput<'a> {
    handles: Vec<LightHandle>,
    geometry: GeometryTransform,
    lens_geometry: Option<&'a LensGeometryMap>,
}

impl<'a> LightHandleInput<'a> {
    pub(in crate::ui::preview) fn of(
        ctx: &egui::Context,
        app: &'a CalibRawApp,
        layout: ScreenLayout,
    ) -> Self {
        let card_open =
            crate::ui::sidebar::effect_card_open(ctx, crate::pipeline::MaskEffect::Relight);
        Self {
            handles: app.light_handles(layout, card_open),
            geometry: app.develop.geometry,
            lens_geometry: loaded_lens_geometry(app).map(Arc::as_ref),
        }
    }

    pub(in crate::ui::preview) fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }
}

/// Where a handle sits on screen.
#[derive(Clone, Copy)]
struct HandlePlacement {
    /// The light's own position, possibly outside the viewport.
    light: Pos2,
    /// Where the handle is drawn: the light, kept inside the viewport.
    drawn: Pos2,
}

impl HandlePlacement {
    fn of(light: Pos2, viewport: Rect) -> Self {
        let bounds = viewport.shrink(HIT_RADIUS);
        let drawn = if bounds.is_positive() {
            bounds.clamp(light)
        } else {
            viewport.center()
        };
        Self { light, drawn }
    }

    fn in_view(self) -> bool {
        self.light.distance(self.drawn) < 0.5
    }
}

impl Preview {
    /// Show the light handles over the canvas and return their edits. The
    /// handles are registered after the canvas response, so a press, drag or
    /// scroll on a handle reaches it instead of panning, zooming or editing a
    /// mask.
    pub(in crate::ui::preview) fn light_handle_actions(
        ui: &Ui,
        input: &LightHandleInput<'_>,
        layout: PreviewLayout,
    ) -> Vec<LightHandleAction> {
        let projection = layout.projection(input.geometry, input.lens_geometry);
        let painter = ui.painter_at(layout.viewport_rect);
        let mut actions = Vec::new();
        for handle in &input.handles {
            let light = projection.to_screen([handle.source[0] / 100.0, handle.source[1] / 100.0]);
            let placement = HandlePlacement::of(light, layout.viewport_rect);
            let id = handle_id(ui, handle.target);
            let response = ui.interact(
                Rect::from_center_size(placement.drawn, egui::Vec2::splat(2.0 * HIT_RADIUS)),
                id,
                Sense::click_and_drag(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Other,
                    ui.is_enabled(),
                    format!(
                        "Relight light: {:.0}%, {:.0}%, depth {:.0}",
                        handle.source[0], handle.source[1], handle.depth
                    ),
                )
            });

            let mut source = handle.source;
            let mut depth = handle.depth;
            if response.double_clicked() {
                source = [params::SOURCE_X.default, params::SOURCE_Y.default];
                actions.push(LightHandleAction::Move {
                    target: handle.target,
                    source,
                });
            } else if let Some(moved) = drag_to(ui, &response, id, placement) {
                let uv = projection.to_source(moved);
                source = [
                    params::SOURCE_X.clamp(uv[0] * 100.0),
                    params::SOURCE_Y.clamp(uv[1] * 100.0),
                ];
                if source != handle.source {
                    actions.push(LightHandleAction::Move {
                        target: handle.target,
                        source,
                    });
                }
            }

            let active = response.hovered() || response.dragged();
            if active {
                let (scroll, fine) =
                    ui.input(|input| (input.smooth_scroll_delta.y, input.modifiers.shift));
                if scroll.abs() > 0.01 {
                    let rate = DEPTH_PER_SCROLL_POINT * if fine { 0.2 } else { 1.0 };
                    // Scrolling up pushes the light away from the camera.
                    depth = params::DEPTH.clamp(depth + scroll * rate);
                    if depth != handle.depth {
                        actions.push(LightHandleAction::SetDepth {
                            target: handle.target,
                            depth,
                        });
                    }
                }
                ui.ctx().set_cursor_icon(if response.dragged() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                });
            }

            let placement = if source == handle.source {
                placement
            } else {
                HandlePlacement::of(
                    projection.to_screen([source[0] / 100.0, source[1] / 100.0]),
                    layout.viewport_rect,
                )
            };
            paint_light_handle(&painter, placement, handle.color, depth, active);
            response.on_hover_text(
                "Drag to move the light. Scroll to move it nearer or farther; Shift scrolls finely. Double-click to reset its position.",
            );
        }
        actions
    }
}

fn handle_id(ui: &Ui, target: EffectComponentRef) -> egui::Id {
    ui.id().with(("relight-light-handle", target))
}

/// Where a dragged handle moves to this frame. The light keeps its offset from
/// the pointer, so grabbing it off-center does not make it jump; a light
/// outside the view comes to the pointer.
fn drag_to(
    ui: &Ui,
    response: &egui::Response,
    id: egui::Id,
    placement: HandlePlacement,
) -> Option<Pos2> {
    let pointer = response.interact_pointer_pos()?;
    if response.drag_started() {
        // Measured from the press: the drag starts past a small threshold.
        let pressed_at = ui
            .input(|input| input.pointer.press_origin())
            .unwrap_or(pointer);
        let offset = if placement.in_view() {
            placement.light - pressed_at
        } else {
            egui::Vec2::ZERO
        };
        ui.data_mut(|data| data.insert_temp(id, offset));
    }
    if !response.dragged() {
        return None;
    }
    let offset = ui
        .data(|data| data.get_temp::<egui::Vec2>(id))
        .unwrap_or_default();
    let moved = pointer + offset;
    (moved.distance(placement.light) > 0.01).then_some(moved)
}

fn paint_light_handle(
    painter: &egui::Painter,
    placement: HandlePlacement,
    color: [f32; 3],
    depth: f32,
    active: bool,
) {
    let center = placement.drawn;
    let nearness =
        ((params::DEPTH.max - depth) / (params::DEPTH.max - params::DEPTH.min)).clamp(0.0, 1.0);
    let ring = egui::lerp(RING_RADIUS[1]..=RING_RADIUS[0], nearness);
    let ring_width = if active { 2.0 } else { 1.25 };
    handles::paint_ring(
        painter,
        center,
        ring,
        Stroke::new(ring_width, Color32::WHITE),
    );
    let [r, g, b] = color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
    handles::paint_point(painter, center, CORE_RADIUS, Color32::from_rgb(r, g, b));

    if !placement.in_view() {
        // Point toward a light placed beyond the view.
        let direction = (placement.light - center).normalized();
        let tip = center + direction * (ring + 9.0);
        let base = center + direction * (ring + 3.0);
        let side = direction.rot90() * 4.0;
        painter.add(Shape::convex_polygon(
            vec![tip, base + side, base - side],
            Color32::WHITE,
            Stroke::new(1.0, Color32::from_black_alpha(130)),
        ));
    }

    if active {
        handles::paint_value_label(
            painter,
            center,
            ring + 14.0,
            format!("Depth {}", format_depth(depth)),
        );
    }
}

/// Whole depth with a true minus sign, as the slider shows it.
fn format_depth(depth: f32) -> String {
    let value = depth.round();
    if value < 0.0 {
        format!("\u{2212}{:.0}", -value)
    } else {
        format!("{value:.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: Rect = Rect {
        min: Pos2::ZERO,
        max: Pos2 { x: 400.0, y: 300.0 },
    };

    fn layout() -> PreviewLayout {
        PreviewLayout {
            image_rect: VIEWPORT,
            visible_rect: VIEWPORT,
            viewport_rect: VIEWPORT,
            source_width: 4000,
            source_height: 3000,
        }
    }

    fn input(source: [f32; 2], depth: f32) -> LightHandleInput<'static> {
        LightHandleInput {
            handles: vec![LightHandle {
                target: EffectComponentRef::Global(0),
                source,
                depth,
                color: [1.0, 0.9, 0.8],
            }],
            geometry: GeometryTransform::default(),
            lens_geometry: None,
        }
    }

    /// Runs frames with a canvas response under the handles, as the preview
    /// does, and returns the light actions of the last frame and whether the
    /// canvas saw the pointer.
    struct Canvas {
        ctx: egui::Context,
        time: f64,
    }

    impl Canvas {
        fn new() -> Self {
            Self {
                ctx: egui::Context::default(),
                time: 0.0,
            }
        }

        fn frame(
            &mut self,
            input: &LightHandleInput<'_>,
            events: Vec<egui::Event>,
            shift: bool,
        ) -> (Vec<LightHandleAction>, bool) {
            self.time += 0.05;
            let mut actions = Vec::new();
            let mut canvas_hovered = false;
            let _ = self.ctx.run_ui(
                egui::RawInput {
                    time: Some(self.time),
                    screen_rect: Some(VIEWPORT),
                    events,
                    modifiers: egui::Modifiers {
                        shift,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                |ui| {
                    let canvas =
                        ui.interact(VIEWPORT, ui.id().with("canvas"), Sense::click_and_drag());
                    actions = Preview::light_handle_actions(ui, input, layout());
                    canvas_hovered = canvas.hovered();
                },
            );
            (actions, canvas_hovered)
        }
    }

    fn pointer(position: Pos2, pressed: Option<bool>) -> Vec<egui::Event> {
        let mut events = vec![egui::Event::PointerMoved(position)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        events
    }

    fn scroll(position: Pos2, delta: f32) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, delta),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn dragging_moves_the_light_by_the_pointer_without_a_jump() {
        // The light at 25%, 50% is drawn at (100, 150); grab it 5 points off.
        let input = input([25.0, 50.0], 0.0);
        let mut canvas = Canvas::new();
        let grab = egui::pos2(105.0, 150.0);
        canvas.frame(&input, pointer(grab, None), false);
        canvas.frame(&input, pointer(grab, Some(true)), false);
        let mut moved = None;
        for step in 1..=10 {
            let (actions, _) = canvas.frame(
                &input,
                pointer(grab + egui::vec2(4.0 * step as f32, 0.0), None),
                false,
            );
            for action in actions {
                if let LightHandleAction::Move { source, .. } = action {
                    moved = Some(source);
                }
            }
        }
        // Moved 40 points of a 400-point image: ten percent to the right.
        let source = moved.expect("drag moves the light");
        assert!((source[0] - 35.0).abs() < 0.01, "{source:?}");
        assert!((source[1] - 50.0).abs() < 0.01, "{source:?}");
    }

    #[test]
    fn scrolling_over_a_light_sets_its_depth_instead_of_zooming() {
        let input = input([25.0, 50.0], -30.0);
        let mut canvas = Canvas::new();
        let over = egui::pos2(100.0, 150.0);
        canvas.frame(&input, pointer(over, None), false);
        let mut depth = None;
        let mut canvas_saw_pointer = false;
        for _ in 0..30 {
            let (actions, canvas_hovered) = canvas.frame(&input, scroll(over, 50.0), false);
            canvas_saw_pointer |= canvas_hovered;
            for action in actions {
                if let LightHandleAction::SetDepth { depth: value, .. } = action {
                    depth = Some(value);
                }
            }
        }
        let depth = depth.expect("scroll sets depth");
        assert!(depth > -30.0, "scrolling up moves the light away: {depth}");
        assert!(
            !canvas_saw_pointer,
            "the canvas would zoom on the same scroll"
        );

        // Away from the light the scroll belongs to the canvas.
        let away = egui::pos2(300.0, 60.0);
        let mut light_actions = 0;
        let mut canvas_hovered = false;
        for _ in 0..5 {
            let (actions, hovered) = canvas.frame(&input, scroll(away, 50.0), false);
            light_actions += actions.len();
            canvas_hovered |= hovered;
        }
        assert_eq!(light_actions, 0);
        assert!(canvas_hovered);
    }

    #[test]
    fn a_light_beyond_the_view_is_pinned_to_its_edge_and_comes_to_the_pointer() {
        let placement = HandlePlacement::of(egui::pos2(-200.0, 150.0), VIEWPORT);
        assert!(!placement.in_view());
        assert_eq!(placement.drawn, egui::pos2(HIT_RADIUS, 150.0));
        let inside = HandlePlacement::of(egui::pos2(100.0, 150.0), VIEWPORT);
        assert!(inside.in_view());

        // Source -50% sits 200 points left of the 400-point image.
        let input = input([-50.0, 50.0], 0.0);
        let mut canvas = Canvas::new();
        let edge = placement.drawn;
        canvas.frame(&input, pointer(edge, None), false);
        canvas.frame(&input, pointer(edge, Some(true)), false);
        let target = egui::pos2(60.0, 120.0);
        let mut moved = None;
        for step in 1..=6 {
            let t = step as f32 / 6.0;
            let (actions, _) = canvas.frame(&input, pointer(edge.lerp(target, t), None), false);
            for action in actions {
                if let LightHandleAction::Move { source, .. } = action {
                    moved = Some(source);
                }
            }
        }
        let source = moved.expect("drag brings the light into view");
        assert!((source[0] - 15.0).abs() < 0.01, "{source:?}");
        assert!((source[1] - 40.0).abs() < 0.01, "{source:?}");
    }

    #[test]
    fn depth_labels_use_whole_numbers_and_a_minus_sign() {
        assert_eq!(format_depth(-29.6), "\u{2212}30");
        assert_eq!(format_depth(0.2), "0");
        assert_eq!(format_depth(44.5), "45");
    }
}
