//! On-canvas controls for effects with a place in the photo. Lights and
//! centers are point handles (`point`); Tilt-Shift's focus band has its own
//! handles for position, angle and widths (`band`). Every control edits the
//! same setting, in the same frame, as the effect card's pad or slider.
//!
//! The handles are widgets registered after the canvas response, so a press,
//! drag or scroll on one reaches it instead of panning, zooming or editing a
//! mask.

use super::super::*;
use crate::app::{EffectComponentRef, EffectHandle, EffectHandleAction};
use crate::pipeline::MaskEffect;
use moduwu_design::ScreenLayout;

mod band;
mod point;

/// The effects shown this frame and how the image is projected.
pub(in crate::ui::preview) struct EffectHandleInput<'a> {
    handles: Vec<EffectHandle>,
    geometry: GeometryTransform,
    lens_geometry: Option<&'a LensGeometryMap>,
}

impl<'a> EffectHandleInput<'a> {
    pub(in crate::ui::preview) fn of(
        ctx: &egui::Context,
        app: &'a CalibRawApp,
        layout: ScreenLayout,
    ) -> Self {
        Self {
            handles: app.effect_handles(layout, |effect| {
                crate::ui::sidebar::effect_card_open(ctx, effect)
            }),
            geometry: app.develop.geometry,
            lens_geometry: loaded_lens_geometry(app).map(Arc::as_ref),
        }
    }

    pub(in crate::ui::preview) fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }
}

/// The frame an effect position is stored in, as a percentage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Frame {
    /// The full, uncropped source image.
    Source,
    /// The developed output, after crop and rotation.
    Output,
}

/// Maps effect positions between the screen and their frames.
#[derive(Clone, Copy)]
struct Frames<'a> {
    projection: SourceProjection<'a>,
    /// The developed image on screen; the output frame spans it.
    output: Rect,
    viewport: Rect,
    /// Source pixels, for measures in source pixels and shorter-edge units.
    source_size: egui::Vec2,
}

impl Frames<'_> {
    fn screen_of(self, frame: Frame, percent: [f32; 2]) -> Pos2 {
        let uv = [percent[0] / 100.0, percent[1] / 100.0];
        match frame {
            Frame::Source => self.projection.to_screen(uv),
            Frame::Output => Pos2::new(
                egui::lerp(self.output.x_range(), uv[0]),
                egui::lerp(self.output.y_range(), uv[1]),
            ),
        }
    }

    fn position_at(self, frame: Frame, screen: Pos2) -> [f32; 2] {
        let uv = match frame {
            Frame::Source => self.projection.to_source(screen),
            Frame::Output => [
                (screen.x - self.output.left()) / self.output.width().max(1.0),
                (screen.y - self.output.top()) / self.output.height().max(1.0),
            ],
        };
        [uv[0] * 100.0, uv[1] * 100.0]
    }
}

impl Preview {
    /// Show the effect controls over the canvas and return their edits.
    pub(in crate::ui::preview) fn effect_handle_actions(
        ui: &Ui,
        input: &EffectHandleInput<'_>,
        layout: PreviewLayout,
    ) -> Vec<EffectHandleAction> {
        let frames = Frames {
            projection: layout.projection(input.geometry, input.lens_geometry),
            output: layout.image_rect,
            viewport: layout.viewport_rect,
            source_size: egui::vec2(
                layout.source_width.max(1) as f32,
                layout.source_height.max(1) as f32,
            ),
        };
        let painter = ui.painter_at(layout.viewport_rect);
        let mut actions = Vec::new();
        for handle in &input.handles {
            let edits = match handle.effect {
                MaskEffect::TiltShift => band::show(ui, &painter, frames, handle),
                _ => point::show(ui, &painter, frames, handle),
            };
            actions.extend(edits.into_iter().map(|edit| EffectHandleAction {
                target: handle.target,
                edit,
            }));
        }
        actions
    }
}

fn handle_id(ui: &Ui, target: EffectComponentRef, part: &'static str) -> egui::Id {
    ui.id().with(("effect-handle", target, part))
}

/// The handle's widget: a square of its pointer reach around `center`.
fn grab(ui: &Ui, id: egui::Id, center: Pos2, label: String) -> egui::Response {
    let response = ui.interact(
        Rect::from_center_size(center, egui::Vec2::splat(2.0 * handles::POINT_REACH)),
        id,
        Sense::click_and_drag(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Other, ui.is_enabled(), &label)
    });
    response
}

fn is_active(response: &egui::Response) -> bool {
    response.hovered() || response.dragged()
}

fn set_grab_cursor(ui: &Ui, response: &egui::Response) {
    if response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
}

/// Where a handle sits on screen.
#[derive(Clone, Copy)]
struct Placement {
    /// The position it controls, possibly outside the viewport.
    anchor: Pos2,
    /// Where the handle is drawn: the anchor, kept inside the viewport.
    drawn: Pos2,
}

impl Placement {
    fn of(anchor: Pos2, viewport: Rect) -> Self {
        let bounds = viewport.shrink(handles::POINT_REACH);
        let drawn = if bounds.is_positive() {
            bounds.clamp(anchor)
        } else {
            viewport.center()
        };
        Self { anchor, drawn }
    }

    fn in_view(self) -> bool {
        self.anchor.distance(self.drawn) < 0.5
    }
}

/// Where a dragged handle moves to this frame. The anchor keeps its offset
/// from the pointer, so grabbing a handle off-center does not make it jump;
/// an anchor outside the view comes to the pointer.
fn drag_to(ui: &Ui, response: &egui::Response, id: egui::Id, placement: Placement) -> Option<Pos2> {
    let pointer = response.interact_pointer_pos()?;
    if response.drag_started() {
        // Measured from the press: the drag starts past a small threshold.
        let pressed_at = ui
            .input(|input| input.pointer.press_origin())
            .unwrap_or(pointer);
        let offset = if placement.in_view() {
            placement.anchor - pressed_at
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
    (moved.distance(placement.anchor) > 0.01).then_some(moved)
}

/// A whole number with a true minus sign, as the sliders show it.
fn format_signed(value: f32) -> String {
    let value = value.round();
    if value < 0.0 {
        format!("\u{2212}{:.0}", -value)
    } else {
        format!("{value:.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::EffectHandleEdit;
    use crate::pipeline::{EffectComponent, MaskEffectSettings};

    pub(super) const VIEWPORT: Rect = Rect {
        min: Pos2::ZERO,
        max: Pos2 { x: 400.0, y: 300.0 },
    };

    pub(super) fn layout() -> PreviewLayout {
        PreviewLayout {
            image_rect: VIEWPORT,
            visible_rect: VIEWPORT,
            viewport_rect: VIEWPORT,
            source_width: 4000,
            source_height: 3000,
        }
    }

    pub(super) fn input(
        effect: MaskEffect,
        settings: impl FnOnce(&mut MaskEffectSettings),
    ) -> EffectHandleInput<'static> {
        let mut component = EffectComponent::new(effect);
        settings(&mut component.settings);
        EffectHandleInput {
            handles: vec![EffectHandle {
                target: EffectComponentRef::Global(0),
                effect,
                settings: component.settings,
            }],
            geometry: GeometryTransform::default(),
            lens_geometry: None,
        }
    }

    /// Runs frames with a canvas response under the handles, as the preview
    /// does, and returns the edits of each frame and whether the canvas saw
    /// the pointer.
    pub(super) struct Canvas {
        ctx: egui::Context,
        time: f64,
    }

    impl Canvas {
        pub(super) fn new() -> Self {
            Self {
                ctx: egui::Context::default(),
                time: 0.0,
            }
        }

        pub(super) fn frame(
            &mut self,
            input: &EffectHandleInput<'_>,
            events: Vec<egui::Event>,
        ) -> (Vec<EffectHandleEdit>, bool) {
            self.time += 0.05;
            let mut edits = Vec::new();
            let mut canvas_hovered = false;
            let _ = self.ctx.run_ui(
                egui::RawInput {
                    time: Some(self.time),
                    screen_rect: Some(VIEWPORT),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let canvas =
                        ui.interact(VIEWPORT, ui.id().with("canvas"), Sense::click_and_drag());
                    edits = Preview::effect_handle_actions(ui, input, layout())
                        .into_iter()
                        .map(|action| action.edit)
                        .collect();
                    canvas_hovered = canvas.hovered();
                },
            );
            (edits, canvas_hovered)
        }

        /// Presses at `from`, drags to `to` in `steps` and returns the last
        /// edit of each kind made on the way, in order of first appearance.
        pub(super) fn drag(
            &mut self,
            input: &EffectHandleInput<'_>,
            from: Pos2,
            to: Pos2,
            steps: usize,
        ) -> Vec<EffectHandleEdit> {
            self.frame(input, pointer(from, None));
            self.frame(input, pointer(from, Some(true)));
            let mut last: Vec<EffectHandleEdit> = Vec::new();
            for step in 1..=steps {
                let position = from.lerp(to, step as f32 / steps as f32);
                for edit in self.frame(input, pointer(position, None)).0 {
                    match last
                        .iter_mut()
                        .find(|seen| std::mem::discriminant(*seen) == std::mem::discriminant(&edit))
                    {
                        Some(seen) => *seen = edit,
                        None => last.push(edit),
                    }
                }
            }
            self.frame(input, pointer(to, Some(false)));
            last
        }
    }

    pub(super) fn pointer(position: Pos2, pressed: Option<bool>) -> Vec<egui::Event> {
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

    pub(super) fn scroll(position: Pos2, delta: f32) -> Vec<egui::Event> {
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

    pub(super) fn assert_position(edits: &[EffectHandleEdit], expected: [f32; 2]) {
        let position = edits
            .iter()
            .find_map(|edit| match edit {
                EffectHandleEdit::Position(position) => Some(*position),
                _ => None,
            })
            .expect("a position edit");
        for axis in 0..2 {
            assert!(
                (position[axis] - expected[axis]).abs() < 0.05,
                "{position:?} vs {expected:?}"
            );
        }
    }

    #[test]
    fn output_frame_positions_follow_the_developed_image_not_the_source() {
        // The developed image is drawn at an offset and size of its own.
        let projection = SourceProjection::new(
            Rect::from_min_size(Pos2::new(100.0, 50.0), egui::vec2(200.0, 100.0)),
            GeometryTransform::default(),
            None,
            4000,
            2000,
        );
        let frames = Frames {
            projection,
            output: Rect::from_min_size(Pos2::new(100.0, 50.0), egui::vec2(200.0, 100.0)),
            viewport: VIEWPORT,
            source_size: egui::vec2(4000.0, 2000.0),
        };
        let screen = frames.screen_of(Frame::Output, [25.0, 50.0]);
        assert_eq!(screen, Pos2::new(150.0, 100.0));
        let back = frames.position_at(Frame::Output, screen);
        assert!((back[0] - 25.0).abs() < 1e-4 && (back[1] - 50.0).abs() < 1e-4);
        let source = frames.position_at(
            Frame::Source,
            frames.screen_of(Frame::Source, [-50.0, 150.0]),
        );
        assert!((source[0] + 50.0).abs() < 1e-3 && (source[1] - 150.0).abs() < 1e-3);
    }

    #[test]
    fn signed_labels_use_whole_numbers_and_a_minus_sign() {
        assert_eq!(format_signed(-29.6), "\u{2212}30");
        assert_eq!(format_signed(0.2), "0");
        assert_eq!(format_signed(44.5), "45");
    }
}
