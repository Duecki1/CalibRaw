//! Tilt-Shift's focus band: the sharp band's edges, the feathered transition
//! beyond them and the band's axis are drawn over the photo. Drag the center
//! to move the band, the knob to rotate it (Shift snaps to 15°), an edge
//! handle to widen the sharp band and an outer handle to widen the feather.
//! Double-click a handle to reset what it controls. A focused handle steps
//! with the arrow keys: the center moves, and the knob and edge handles
//! turn or widen with Right or Up; Shift makes the steps finer.
//!
//! Geometry follows `mask_tilt_shift_weight` in tilt_shift.wgsl: the band is a
//! straight line in source pixels through the center at the angle, and widths
//! are percentages of the shorter source edge.

use super::*;
use crate::app::EffectHandleEdit;
use crate::i18n::tr;
use crate::pipeline::effect_params::tilt_shift as params;
use crate::pipeline::TiltShiftEffectSettings;

/// Screen distance from the band center to its rotation knob.
const KNOB_DISTANCE: f32 = 64.0;
/// Shift-drag snaps the angle to multiples of this, in degrees.
const ANGLE_SNAP: f32 = 15.0;
/// Points sampled along each band line; lens correction may bend them.
const LINE_SAMPLES: usize = 48;

/// The band in source pixels.
#[derive(Clone, Copy)]
struct Band {
    center: egui::Vec2,
    along: egui::Vec2,
    across: egui::Vec2,
    short_edge: f32,
    size: egui::Vec2,
}

impl Band {
    fn of(settings: &TiltShiftEffectSettings, size: egui::Vec2) -> Self {
        let (sine, cosine) = settings.angle.to_radians().sin_cos();
        Self {
            center: egui::vec2(settings.center[0], settings.center[1]) / 100.0 * size,
            along: egui::vec2(cosine, sine),
            across: egui::vec2(-sine, cosine),
            short_edge: size.x.min(size.y),
            size,
        }
    }

    /// The source position (percent) `along` pixels along the band and
    /// `across` percent of the shorter edge across it.
    fn point(&self, along: f32, across: f32) -> [f32; 2] {
        let pixels =
            self.center + self.along * along + self.across * (across / 100.0 * self.short_edge);
        [
            pixels.x / self.size.x * 100.0,
            pixels.y / self.size.y * 100.0,
        ]
    }

    fn pixels(&self, position: [f32; 2]) -> egui::Vec2 {
        egui::vec2(position[0], position[1]) / 100.0 * self.size
    }

    /// Signed distance of `position` from the band axis, in percent of the
    /// shorter edge.
    fn offset_across(&self, position: [f32; 2]) -> f32 {
        (self.pixels(position) - self.center).dot(self.across) / self.short_edge * 100.0
    }

    /// The angle that points the band, and so its knob, at `position`. The
    /// band is the same at a ± 180°; the knob follows the pointer.
    fn angle_toward(&self, position: [f32; 2], current: f32, snap: bool) -> f32 {
        let delta = self.pixels(position) - self.center;
        if delta.length_sq() < 1e-6 {
            return current;
        }
        let mut angle = delta.y.atan2(delta.x).to_degrees();
        if snap {
            angle = (angle / ANGLE_SNAP).round() * ANGLE_SNAP;
        }
        wrap_degrees(angle)
    }

    /// The line `across` percent of the shorter edge from the axis, on screen.
    fn line(&self, frames: Frames<'_>, across: f32) -> Vec<Pos2> {
        let reach = self.size.length();
        (0..=LINE_SAMPLES)
            .map(|index| {
                let along = -reach + 2.0 * reach * index as f32 / LINE_SAMPLES as f32;
                frames.screen_of(Frame::Source, self.point(along, across))
            })
            .collect()
    }
}

/// Wraps degrees into the parameter range (-180, 180].
fn wrap_degrees(angle: f32) -> f32 {
    let wrapped = (angle + 180.0).rem_euclid(360.0) - 180.0;
    if wrapped <= -180.0 {
        wrapped + 360.0
    } else {
        wrapped
    }
}

/// One of the band's handles.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Feather(Side),
    Focus(Side),
    Knob,
    Center,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Near,
    Far,
}

impl Side {
    fn sign(self) -> f32 {
        match self {
            Self::Near => -1.0,
            Self::Far => 1.0,
        }
    }
}

impl Part {
    /// Registration order: later parts lie above earlier ones, so the center
    /// stays movable when a narrow band puts the edge handles on it.
    const ALL: [Self; 6] = [
        Self::Feather(Side::Near),
        Self::Feather(Side::Far),
        Self::Focus(Side::Near),
        Self::Focus(Side::Far),
        Self::Knob,
        Self::Center,
    ];

    fn id_part(self) -> &'static str {
        match self {
            Self::Feather(Side::Near) => "tilt-shift-feather-near",
            Self::Feather(Side::Far) => "tilt-shift-feather-far",
            Self::Focus(Side::Near) => "tilt-shift-focus-near",
            Self::Focus(Side::Far) => "tilt-shift-focus-far",
            Self::Knob => "tilt-shift-knob",
            Self::Center => "tilt-shift-center",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Feather(_) => tr("Feather"),
            Self::Focus(_) => tr("Focus Width"),
            Self::Knob => tr("Angle"),
            Self::Center => tr("Focus position"),
        }
    }

    /// The label drawn beside a hovered or dragged handle.
    fn label(self, settings: &TiltShiftEffectSettings) -> String {
        match self {
            Self::Center => self.name().to_owned(),
            _ => format!("{} {}", self.name(), self.value(settings)),
        }
    }

    fn value(self, settings: &TiltShiftEffectSettings) -> String {
        match self {
            Self::Feather(_) => format!("{:.0}", settings.feather),
            Self::Focus(_) => format!("{:.0}", settings.focus_width),
            Self::Knob => format!("{}°", format_signed(settings.angle)),
            Self::Center => format!("{:.0}%, {:.0}%", settings.center[0], settings.center[1]),
        }
    }
}

/// Where each handle sits on screen for `settings`.
struct Layout {
    band: Band,
    center: Placement,
    knob: Pos2,
}

impl Layout {
    fn of(settings: &TiltShiftEffectSettings, frames: Frames<'_>) -> Self {
        let band = Band::of(settings, frames.source_size);
        let anchor = frames.screen_of(Frame::Source, settings.center);
        let center = Placement::of(anchor, frames.viewport);
        let toward = frames.screen_of(Frame::Source, band.point(band.short_edge * 0.05, 0.0));
        let direction = (toward - anchor).normalized();
        let direction = if direction.is_finite() && direction != egui::Vec2::ZERO {
            direction
        } else {
            egui::Vec2::X
        };
        Self {
            band,
            center,
            knob: center.drawn + direction * KNOB_DISTANCE,
        }
    }

    fn edge(&self, settings: &TiltShiftEffectSettings, frames: Frames<'_>, part: Part) -> Pos2 {
        let half = settings.focus_width * 0.5;
        let across = match part {
            Part::Focus(side) => side.sign() * half,
            Part::Feather(side) => side.sign() * (half + settings.feather),
            Part::Knob => return self.knob,
            Part::Center => return self.center.drawn,
        };
        frames.screen_of(Frame::Source, self.band.point(0.0, across))
    }
}

pub(super) fn show(
    ui: &Ui,
    painter: &egui::Painter,
    frames: Frames<'_>,
    handle: &EffectHandle,
) -> Vec<EffectHandleEdit> {
    let original = handle.settings.tilt_shift;
    let mut settings = original;
    let layout = Layout::of(&original, frames);
    let reachable = frames.viewport.shrink(handles::POINT_REACH * 0.5);
    let mut edits = Vec::new();
    let mut active_part = None;
    for part in Part::ALL {
        let screen = layout.edge(&original, frames, part);
        if part != Part::Center && !reachable.contains(screen) {
            continue;
        }
        let id = handle_id(ui, handle.target, part.id_part());
        let response = grab(ui, id, screen, part.name(), part.value(&original));
        if let Some(edit) = edit_for(ui, &response, id, part, screen, &layout, frames, &settings) {
            apply(&mut settings, edit);
            edits.push(edit);
        }
        set_grab_cursor(ui, &response);
        if is_active(&response) {
            active_part = Some(part);
        }
    }
    paint(painter, frames, &settings, active_part);
    edits
}

#[allow(clippy::too_many_arguments)]
fn edit_for(
    ui: &Ui,
    response: &egui::Response,
    id: egui::Id,
    part: Part,
    screen: Pos2,
    layout: &Layout,
    frames: Frames<'_>,
    settings: &TiltShiftEffectSettings,
) -> Option<EffectHandleEdit> {
    if moduwu_design::double_tapped(response) {
        return Some(match part {
            Part::Center => {
                EffectHandleEdit::Position([params::CENTER_X.default, params::CENTER_Y.default])
            }
            Part::Knob => EffectHandleEdit::Angle(params::ANGLE.default),
            Part::Focus(_) => EffectHandleEdit::FocusWidth(params::FOCUS_WIDTH.default),
            Part::Feather(_) => EffectHandleEdit::Feather(params::FEATHER.default),
        });
    }
    let placement = match part {
        Part::Center => layout.center,
        _ => Placement {
            anchor: screen,
            drawn: screen,
        },
    };
    let Some(moved) = drag_to(ui, response, id, placement) else {
        return key_edit(ui, response, part, settings);
    };
    let position = frames.position_at(Frame::Source, moved);
    let edit = match part {
        Part::Center => EffectHandleEdit::Position([
            params::CENTER_X.clamp(position[0]),
            params::CENTER_Y.clamp(position[1]),
        ]),
        Part::Knob => {
            let snap = ui.input(|input| input.modifiers.shift);
            EffectHandleEdit::Angle(layout.band.angle_toward(position, settings.angle, snap))
        }
        Part::Focus(_) => EffectHandleEdit::FocusWidth(
            params::FOCUS_WIDTH.clamp(2.0 * layout.band.offset_across(position).abs()),
        ),
        Part::Feather(_) => EffectHandleEdit::Feather(
            params::FEATHER
                .clamp(layout.band.offset_across(position).abs() - settings.focus_width * 0.5),
        ),
    };
    (edit != current_value(settings, part)).then_some(edit)
}

/// The edit of arrow keys on a focused handle: the center moves, the other
/// handles turn or widen with Right or Up and the reverse with Left or Down.
fn key_edit(
    ui: &Ui,
    response: &egui::Response,
    part: Part,
    settings: &TiltShiftEffectSettings,
) -> Option<EffectHandleEdit> {
    let steps = arrow_steps(ui, response);
    let increase = steps.x - steps.y;
    let edit = match part {
        Part::Center => EffectHandleEdit::Position([
            params::CENTER_X.clamp(settings.center[0] + steps.x),
            params::CENTER_Y.clamp(settings.center[1] + steps.y),
        ]),
        Part::Knob => EffectHandleEdit::Angle(wrap_degrees(settings.angle + increase)),
        Part::Focus(_) => {
            EffectHandleEdit::FocusWidth(params::FOCUS_WIDTH.clamp(settings.focus_width + increase))
        }
        Part::Feather(_) => {
            EffectHandleEdit::Feather(params::FEATHER.clamp(settings.feather + increase))
        }
    };
    (edit != current_value(settings, part)).then_some(edit)
}

fn current_value(settings: &TiltShiftEffectSettings, part: Part) -> EffectHandleEdit {
    match part {
        Part::Center => EffectHandleEdit::Position(settings.center),
        Part::Knob => EffectHandleEdit::Angle(settings.angle),
        Part::Focus(_) => EffectHandleEdit::FocusWidth(settings.focus_width),
        Part::Feather(_) => EffectHandleEdit::Feather(settings.feather),
    }
}

fn apply(settings: &mut TiltShiftEffectSettings, edit: EffectHandleEdit) {
    match edit {
        EffectHandleEdit::Position(position) => settings.center = position,
        EffectHandleEdit::Angle(angle) => settings.angle = angle,
        EffectHandleEdit::FocusWidth(width) => settings.focus_width = width,
        EffectHandleEdit::Feather(width) => settings.feather = width,
        EffectHandleEdit::Depth(_) => {}
    }
}

fn paint(
    painter: &egui::Painter,
    frames: Frames<'_>,
    settings: &TiltShiftEffectSettings,
    active: Option<Part>,
) {
    let layout = Layout::of(settings, frames);
    let band = layout.band;
    let half = settings.focus_width * 0.5;
    let outer = half + settings.feather;
    let guide = |across: f32, width: f32, alpha: u8, dashed: bool| {
        let points = band.line(frames, across);
        let stroke = Stroke::new(width, Color32::from_white_alpha(alpha));
        if dashed {
            painter.extend(Shape::dashed_line(&points, stroke, 6.0, 5.0));
        } else {
            handles::paint_guide(painter, points, stroke);
        }
    };
    guide(0.0, 1.0, 110, true);
    for sign in [-1.0, 1.0] {
        guide(sign * outer, 1.0, 150, true);
        guide(sign * half, 1.5, 235, false);
    }

    for part in Part::ALL {
        let screen = layout.edge(settings, frames, part);
        match part {
            Part::Feather(_) => handles::paint_ring(
                painter,
                screen,
                handles::SECONDARY_POINT_RADIUS + 0.5,
                Stroke::new(1.5, Color32::WHITE),
            ),
            Part::Focus(_) => handles::paint_point(
                painter,
                screen,
                handles::SECONDARY_POINT_RADIUS,
                Color32::WHITE,
            ),
            Part::Knob => {
                handles::paint_stem(painter, layout.center.drawn, screen, Color32::WHITE);
                handles::paint_ring(
                    painter,
                    screen,
                    handles::ROTATION_RADIUS,
                    Stroke::new(2.0, Color32::WHITE),
                );
            }
            Part::Center => {
                handles::paint_point(painter, screen, handles::POINT_RADIUS, Color32::WHITE)
            }
        }
        if active == Some(part) {
            handles::paint_value_label(painter, screen, 18.0, part.label(settings));
        }
    }
}
