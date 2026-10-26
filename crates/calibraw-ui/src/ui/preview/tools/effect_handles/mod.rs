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

/// Keyboard steps of a handle: one step per arrow press, a fifth of a step
/// with Shift, as Shift scrolls finely.
const FINE_STEP: f32 = 0.2;

/// The handle's widget: a square of its pointer reach around `center`. It
/// takes keyboard focus when pressed, so arrow keys can nudge it afterwards,
/// and tells assistive technology its `name` and current `value`.
fn grab(ui: &Ui, id: egui::Id, center: Pos2, name: &str, value: String) -> egui::Response {
    let response = ui.interact(
        Rect::from_center_size(center, egui::Vec2::splat(2.0 * handles::POINT_REACH)),
        id,
        Sense::click_and_drag(),
    );
    if response.drag_started() || response.clicked() {
        response.request_focus();
    }
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::WidgetType::Other, ui.is_enabled(), name);
        info.current_text_value = Some(value.clone());
        info
    });
    response
}

/// Arrow presses on a focused handle this frame, as steps right and down.
fn arrow_steps(ui: &Ui, response: &egui::Response) -> egui::Vec2 {
    use egui::Key;
    let presses = |key| key_steps(ui, response, key);
    egui::vec2(
        presses(Key::ArrowRight) - presses(Key::ArrowLeft),
        presses(Key::ArrowDown) - presses(Key::ArrowUp),
    )
}

/// Page Up presses minus Page Down presses on a focused handle this frame.
fn page_steps(ui: &Ui, response: &egui::Response) -> f32 {
    key_steps(ui, response, egui::Key::PageUp) - key_steps(ui, response, egui::Key::PageDown)
}

/// Presses of `key` on a focused handle this frame, a fifth of a step each
/// with Shift. They are consumed so they do not also browse photos.
fn key_steps(ui: &Ui, response: &egui::Response, key: egui::Key) -> f32 {
    use egui::Modifiers;
    if !response.has_focus() {
        return 0.0;
    }
    ui.input_mut(|input| {
        // Shift first: an unmodified key also matches presses with Shift.
        [(Modifiers::SHIFT, FINE_STEP), (Modifiers::NONE, 1.0)]
            .into_iter()
            .map(|(modifiers, scale)| input.count_and_consume_key(modifiers, key) as f32 * scale)
            .sum()
    })
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
